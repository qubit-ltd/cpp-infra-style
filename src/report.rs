// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Shared report identity, command capture, artifacts and atomic JSON output.

use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Instant;

use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use tempfile::Builder;
use tempfile::NamedTempFile;

use crate::scope::digest;
use crate::scope::hash;
use crate::scope::protocol_path;
use crate::style_error::StyleError;

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn tool() -> Value {
    json!({"name":"cpp-infra-style", "version":env!("CARGO_PKG_VERSION"), "revision":option_env!("CPP_INFRA_BUILD_REVISION")})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunContext {
    schema: u32,
    run_id: String,
    task_id: String,
    bundle_id: String,
    project: PathBuf,
    profile: Option<String>,
    config_digest: String,
    source_digest: String,
    report_root: PathBuf,
    #[serde(default)]
    artifacts: Vec<Value>,
    build_context: Option<Value>,
    runtime_snapshot: Option<PathBuf>,
    runtime_snapshot_digest: Option<String>,
    #[serde(default)]
    source_excluded_paths: Vec<String>,
}

pub(crate) struct Report {
    pub value: Value,
    pub root: PathBuf,
    pub output: PathBuf,
    pub started: Instant,
    pub excluded: Vec<PathBuf>,
    invocation_dir: Option<PathBuf>,
}

impl Report {
    /// Append to a report collection while preserving the caller's diagnostic
    /// if an internal report invariant has been violated.
    pub fn append(&mut self, collection: &str, entry: Value, failure_message: &str) -> Result<(), StyleError> {
        match self.value[collection].as_array_mut() {
            Some(entries) => {
                entries.push(entry);
                Ok(())
            }
            None => Err(StyleError::Infrastructure(failure_message.into())),
        }
    }

    pub fn new(project: &Path, output: Option<&Path>) -> Self {
        let run_id = format!(
            "{}-{}-{}",
            Utc::now().format("%Y%m%dT%H%M%S%3fZ"),
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let root = project.join("build/infra-runs").join(&run_id);
        let output = output.map_or_else(|| root.join("style-report.json"), Path::to_path_buf);
        Self {
            value: json!({"schema":1,"tool":tool(),"bundle_id":"development","run_id":run_id,"task_id":"style","project":project,"report_root":root,"source_revision":null,"source_digest":null,"config_digest":null,"status":"error","started_at":Utc::now().to_rfc3339(),"duration_ms":0,"external_versions":{},"commands":[],"diagnostics":[],"artifacts":[],"data":{"changed_files":[],"unfinished_files":[],"exceptions":[]}}),
            root,
            output,
            started: Instant::now(),
            excluded: Vec::new(),
            invocation_dir: None,
        }
    }

    pub fn initialize(
        &mut self,
        project: &Path,
        context_path: Option<&Path>,
        loaded: &crate::config::Loaded,
    ) -> Result<(), StyleError> {
        let context: Option<RunContext> = context_path
            .map(|path| -> Result<RunContext, StyleError> {
                serde_json::from_slice(&fs::read(path)?)
                    .map_err(|e| StyleError::Configuration(format!("invalid run context: {e}")))
            })
            .transpose()?;
        if let Some(context) = &context {
            if context.schema != 1
                || context.project != project
                || context.run_id.trim().is_empty()
                || context.task_id.trim().is_empty()
                || context.bundle_id.trim().is_empty()
                || context.run_id.contains(['/', '\\'])
            {
                return Err(StyleError::Configuration("run context identity mismatch".into()));
            }
            if !context.report_root.is_absolute()
                || !context.report_root.starts_with(project.join("build/infra-runs"))
                || context
                    .report_root
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(StyleError::Configuration(
                    "run context report_root outside build/infra-runs".into(),
                ));
            }
            self.root = context.report_root.clone();
            self.value["report_root"] = json!(self.root);
            self.value["run_id"] = json!(context.run_id);
            self.value["task_id"] = json!(context.task_id);
            self.value["bundle_id"] = json!(context.bundle_id);
            self.value["data"]["profile"] = json!(context.profile);
            self.value["data"]["input_artifacts"] = json!(context.artifacts);
            self.value["data"]["build_context"] = json!(context.build_context);
            validate_runtime_snapshot(context)?;
            self.value["data"]["runtime_snapshot"] = json!(context.runtime_snapshot);
            self.value["data"]["runtime_snapshot_digest"] = json!(context.runtime_snapshot_digest);
        }
        let inherited = context
            .as_ref()
            .map_or(&[][..], |context| context.source_excluded_paths.as_slice());
        let exclusions = crate::source_exclusions::merge(project, loaded, &self.output, inherited)?;
        self.excluded = exclusions.iter().map(|relative| project.join(relative)).collect();
        self.value["source_excluded_paths"] = json!(exclusions);
        let source = digest(project, false, &self.excluded)?;
        let config = digest(project, true, &self.excluded)?;
        self.value["source_digest"] = json!(source);
        self.value["config_digest"] = json!(config);
        if let Some(context) = &context
            && (context.source_digest != source || context.config_digest != config)
        {
            return Err(StyleError::Configuration(
                "run context is stale: source/config digest mismatch".into(),
            ));
        }
        controlled_directory(&self.root)?;
        let style_root = self.root.join("style");
        controlled_directory(&style_root)?;
        self.invocation_dir = Some(Builder::new().prefix("call-").tempdir_in(style_root)?.keep());
        Ok(())
    }

    pub fn create_log(&self, number: usize) -> Result<(File, String), StyleError> {
        let directory = self
            .invocation_dir
            .as_ref()
            .ok_or_else(|| StyleError::Infrastructure("report invocation directory is not initialized".into()))?;
        validate_ancestors(directory)?;
        let path = directory.join(format!("command-{number}.log"));
        let file = OpenOptions::new().write(true).create_new(true).open(&path)?;
        let relative = protocol_path(&self.root, &path)?;
        Ok((file, relative))
    }

    pub fn artifact(&mut self, kind: &str, relative: &str, bytes: &[u8]) -> Result<(), StyleError> {
        crate::config::relative(relative)?;
        let directory = self
            .invocation_dir
            .as_ref()
            .ok_or_else(|| StyleError::Infrastructure("report invocation directory is not initialized".into()))?;
        validate_ancestors(directory)?;
        let path = directory.join(relative);
        let mut file = NamedTempFile::new_in(directory)?;
        file.write_all(bytes)?;
        file.as_file().sync_all()?;
        validate_ancestors(directory)?;
        file.persist_noclobber(&path).map_err(|error| {
            StyleError::Infrastructure(format!("create immutable artifact {}: {}", path.display(), error.error))
        })?;
        let relative = protocol_path(&self.root, &path)?;
        self.append(
            "artifacts",
            json!({"kind":kind,"path":relative,"sha256":hash(bytes),"required":true}),
            "invalid internal artifacts state",
        )?;
        Ok(())
    }

    pub fn finish(&mut self, error: Option<&StyleError>, failed: bool) -> Result<(), StyleError> {
        self.value["duration_ms"] = json!(self.started.elapsed().as_millis());
        self.value["status"] = json!(match error {
            Some(StyleError::Cancelled) => "cancelled",
            Some(_) => "error",
            None if failed => "failed",
            None => "passed",
        });
        if let Some(error) = error {
            self.append("diagnostics",json!({"code":match error {StyleError::Configuration(_) => "STYLE_CONFIG",StyleError::ConcurrentModification(_) => "STYLE_CONCURRENT_EDIT",StyleError::Infrastructure(_) => "STYLE_INFRASTRUCTURE",StyleError::Cancelled => "STYLE_CANCELLED"},"severity":"error","message":error.to_string(),"project_relative_path":null,"line":null,"column":null,"rule":null,"suppression_reason":null}),"invalid internal diagnostics state")?;
        }
        let bytes = serde_json::to_vec_pretty(&self.value).map_err(StyleError::infrastructure_message)?;
        crate::source_exclusions::existing_report(&self.output)?;
        atomic_json_or_bytes(&self.output, &bytes)
    }
}

pub(crate) fn validate_ancestors(path: &Path) -> Result<(), StyleError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(StyleError::Configuration(
            "output path must be normalized and absolute".into(),
        ));
    }
    let mut ancestor = path;
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| StyleError::Configuration("output path has no existing ancestor".into()))?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    if fs::canonicalize(ancestor)? != ancestor {
        return Err(StyleError::Configuration(format!(
            "output ancestor contains a symbolic link: {}",
            ancestor.display()
        )));
    }
    Ok(())
}

fn controlled_directory(path: &Path) -> Result<(), StyleError> {
    validate_ancestors(path)?;
    fs::create_dir_all(path)?;
    validate_ancestors(path)
}

pub(crate) fn atomic_json_or_bytes(path: &Path, bytes: &[u8]) -> Result<(), StyleError> {
    validate_ancestors(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| StyleError::Configuration("output has no parent".into()))?;
    controlled_directory(parent)?;
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(StyleError::Configuration(format!(
            "refusing symlink output {}",
            path.display()
        )));
    }
    let mut file = NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    validate_ancestors(parent)?;
    file.persist(path)
        .map_err(|e| StyleError::Infrastructure(format!("write {}: {}", path.display(), e.error)))?;
    Ok(())
}

fn validate_runtime_snapshot(context: &RunContext) -> Result<(), StyleError> {
    let (path, expected) = match (&context.runtime_snapshot, &context.runtime_snapshot_digest) {
        (None, None) => return Ok(()),
        (Some(path), Some(expected)) => (path, expected),
        _ => {
            return Err(StyleError::Configuration(
                "runtime snapshot path and digest must appear together".into(),
            ));
        }
    };
    if !path.is_absolute() {
        return Err(StyleError::Configuration(
            "runtime snapshot path must be absolute".into(),
        ));
    }
    let bytes = fs::read(path)?;
    if hash(&bytes) != *expected {
        return Err(StyleError::Configuration("runtime snapshot digest mismatch".into()));
    }
    let snapshot: Value = serde_json::from_slice(&bytes)
        .map_err(|e| StyleError::Configuration(format!("invalid runtime snapshot: {e}")))?;
    if snapshot["schema"] != 1 || snapshot["bundle_id"] != context.bundle_id {
        return Err(StyleError::Configuration(
            "runtime snapshot bundle/schema mismatch".into(),
        ));
    }
    let tools = snapshot["tools"]
        .as_array()
        .ok_or_else(|| StyleError::Configuration("runtime snapshot tools missing".into()))?;
    let selected: Vec<&Value> = tools
        .iter()
        .filter(|entry| entry["name"] == "cpp-infra-style")
        .collect();
    if selected.len() != 1 {
        return Err(StyleError::Configuration(
            "runtime snapshot must identify cpp-infra-style exactly once".into(),
        ));
    }
    let entry = selected[0];
    let identity = tool();
    if ["name", "version", "revision", "protocol", "binary", "executable_sha256"]
        .iter()
        .any(|field| entry.get(field).is_none())
        || entry["version"] != identity["version"]
        || entry["revision"] != identity["revision"]
        || entry["protocol"] != 1
    {
        return Err(StyleError::Configuration(
            "runtime snapshot tool identity mismatch".into(),
        ));
    }
    let binary = entry["binary"]
        .as_str()
        .map(Path::new)
        .ok_or_else(|| StyleError::Configuration("runtime snapshot binary path missing".into()))?;
    if !binary.is_absolute() || !binary.is_file() {
        return Err(StyleError::Configuration(
            "runtime snapshot binary must be an existing absolute file".into(),
        ));
    }
    let expected = entry["executable_sha256"]
        .as_str()
        .ok_or_else(|| StyleError::Configuration("runtime snapshot executable digest missing".into()))?;
    if hash(&fs::read(binary)?) != expected || hash(&fs::read(std::env::current_exe()?)?) != expected {
        return Err(StyleError::Configuration(
            "runtime snapshot executable digest mismatch".into(),
        ));
    }
    Ok(())
}
