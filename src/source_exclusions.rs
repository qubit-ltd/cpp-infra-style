// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Exact report-file exclusions without hiding project inputs or directories.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::config::Loaded;
use crate::config::confined;
use crate::config::relative;
use crate::scope::protocol_path;
use crate::style_error::StyleError;

pub(crate) fn existing_report(path: &Path) -> Result<(), StyleError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(StyleError::Configuration("report path must name a regular file".into()));
    }
    let report: Value = serde_json::from_slice(&fs::read(path)?)
        .map_err(|_| StyleError::Configuration("report path contains an unknown business file".into()))?;
    let family = [
        "cpp-infra-tools",
        "cpp-infra-ci",
        "cpp-infra-verify",
        "cpp-infra-style",
        "cpp-infra-analyze",
        "cpp-infra-dependency",
        "cpp-infra-coverage",
        "cpp-infra-pages",
    ];
    let required = [
        "schema",
        "tool",
        "bundle_id",
        "run_id",
        "task_id",
        "project",
        "report_root",
        "source_revision",
        "source_digest",
        "config_digest",
        "status",
        "started_at",
        "duration_ms",
        "external_versions",
        "commands",
        "diagnostics",
        "artifacts",
    ];
    if report["schema"] != 1
        || !report["tool"]["name"]
            .as_str()
            .is_some_and(|name| family.contains(&name))
        || required.iter().any(|field| report.get(field).is_none())
        || report["tool"].get("revision").is_none()
        || report["tool"]["version"].as_str().is_none_or(str::is_empty)
        || !report["commands"].is_array()
        || !report["diagnostics"].is_array()
        || !report["artifacts"].is_array()
    {
        return Err(StyleError::Configuration(
            "report path contains an unknown business file, not a public cpp-infra report".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_file(project: &Path, loaded: &Loaded, path: &Path) -> Result<Option<String>, StyleError> {
    crate::report::validate_ancestors(path)?;
    if path.extension().and_then(|part| part.to_str()) != Some("json") {
        return Err(StyleError::Configuration(
            "source_excluded_paths requires JSON report files".into(),
        ));
    }
    let relative = if path.starts_with(project) {
        Some(protocol_path(project, path)?)
    } else {
        None
    };
    if let Some(relative) = &relative {
        let first = relative.split('/').next().unwrap_or("").to_ascii_lowercase();
        if [".git", ".infra", "conan", "cmake"].contains(&first.as_str())
            || first.starts_with("readme")
            || first.starts_with("cmake")
            || first.starts_with("conan")
        {
            return Err(StyleError::Configuration(format!(
                "source_excluded_paths cannot hide a reserved project input: {relative}"
            )));
        }
        for owned in loaded
            .project
            .source_roots
            .iter()
            .chain(&loaded.project.test_roots)
            .chain(&loaded.project.generated_roots)
            .chain(&loaded.project.third_party_roots)
        {
            if path.starts_with(confined(project, owned)?) {
                return Err(StyleError::Configuration(format!(
                    "source_excluded_paths cannot hide a configured input: {relative}"
                )));
            }
        }
    }
    existing_report(path)?;
    Ok(relative)
}

pub(crate) fn merge(
    project: &Path,
    loaded: &Loaded,
    output: &Path,
    inherited: &[String],
) -> Result<Vec<String>, StyleError> {
    let mut selected = BTreeSet::new();
    for entry in inherited {
        let bytes = entry.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            return Err(StyleError::Configuration(
                "source_excluded_paths entries cannot contain drive paths".into(),
            ));
        }
        relative(entry)
            .map_err(|error| StyleError::Configuration(format!("invalid source_excluded_paths entry: {error}")))?;
        let path = project.join(entry);
        if protocol_path(project, &path)? != *entry {
            return Err(StyleError::Configuration(
                "source_excluded_paths entries must be normalized exact file paths".into(),
            ));
        }
        validate_file(project, loaded, &path)?;
        selected.insert(entry.clone());
    }
    if let Some(relative) = validate_file(project, loaded, output)?
        && !matches!(relative.split('/').next(), Some("build" | "target"))
    {
        selected.insert(relative);
    }
    Ok(selected.into_iter().collect())
}
