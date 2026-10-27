// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Strict project scope, rule policy, and exception configuration.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use globset::Glob;
use globset::GlobSet;
use globset::GlobSetBuilder;
use serde::Deserialize;

use crate::configuration_snapshot::ConfigurationSnapshot;
use crate::style_error::StyleError;

pub(crate) const RULES: [&str; 4] = ["format", "spdx-header", "final-newline", "trailing-whitespace"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Project {
    pub schema: u32,
    pub kind: String,
    #[serde(default)]
    pub source_roots: Vec<String>,
    #[serde(default)]
    pub test_roots: Vec<String>,
    #[serde(default)]
    pub generated_roots: Vec<String>,
    #[serde(default)]
    pub third_party_roots: Vec<String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    pub configure: String,
    pub build: String,
    pub test: Option<String>,
    pub conan_context: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    pub schema: u32,
    pub extensions: Vec<String>,
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    pub rules: Vec<String>,
    pub exceptions: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    pub schema: u32,
    pub required_rules: Vec<String>,
    pub formatter_major: u32,
    pub spdx_license: String,
    pub copyright: String,
    pub allow_bom: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exceptions {
    schema: u32,
    #[serde(default)]
    exceptions: Vec<Exception>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Exception {
    pub rule: String,
    pub path: String,
    pub reason: String,
}

pub(crate) struct Loaded {
    pub project: Project,
    pub config: Config,
    pub policy: Policy,
    pub exceptions: Vec<Exception>,
    pub include: GlobSet,
    pub exclude: GlobSet,
    pub formatter_config: PathBuf,
    inputs: Vec<ConfigurationSnapshot>,
}

impl Loaded {
    pub fn verify_inputs(&self, project: &Path) -> Result<(), StyleError> {
        for input in &self.inputs {
            input.verify(project)?;
        }
        Ok(())
    }

    pub fn is_configuration_input(&self, path: &Path) -> bool {
        self.inputs.iter().any(|input| input.matches_path(path))
    }
}

fn read_toml<T: serde::de::DeserializeOwned>(
    project: &Path,
    relative: &str,
    inputs: &mut Vec<ConfigurationSnapshot>,
) -> Result<T, StyleError> {
    let (snapshot, bytes) = ConfigurationSnapshot::capture(project, relative)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| StyleError::Configuration(format!("non-UTF8 configuration {relative}: {error}")))?;
    let value =
        toml::from_str(text).map_err(|error| StyleError::Configuration(format!("invalid {relative}: {error}")))?;
    inputs.push(snapshot);
    Ok(value)
}

pub(crate) fn relative(path: &str) -> Result<(), StyleError> {
    if path.is_empty()
        || path.contains('\\')
        || Path::new(path).components().any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(StyleError::Configuration(format!(
            "expected normalized project-relative path: {path}"
        )));
    }
    Ok(())
}

pub(crate) fn confined(project: &Path, path: &str) -> Result<PathBuf, StyleError> {
    relative(path)?;
    let resolved = fs::canonicalize(project.join(path))
        .map_err(|e| StyleError::Configuration(format!("invalid project path {path}: {e}")))?;
    if !resolved.starts_with(project) {
        return Err(StyleError::Configuration(format!("path escapes project: {path}")));
    }
    Ok(resolved)
}

/// Active configuration must be a tracked regular input. Public fingerprints
/// deliberately omit symlink identity, so links in configuration paths are
/// rejected instead of silently certifying a policy that can be retargeted.
pub(crate) fn configuration_input(project: &Path, path: &str) -> Result<PathBuf, StyleError> {
    relative(path)?;
    let lexical = project.join(path);
    validate_tracked_input_location(project, &lexical)?;
    let mut component_path = project.to_path_buf();
    for component in Path::new(path).components() {
        component_path.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&component_path).map_err(|error| {
            StyleError::Configuration(format!(
                "cannot inspect active configuration input {}: {error}",
                component_path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(StyleError::Configuration(format!(
                "active configuration input must not contain symbolic links: {}",
                component_path.display()
            )));
        }
    }
    let canonical = confined(project, path)?;
    validate_tracked_input_location(project, &canonical)?;
    if canonical != lexical || !canonical.is_file() {
        return Err(StyleError::Configuration(format!(
            "active configuration input must be a directly tracked regular file: {path}"
        )));
    }
    Ok(canonical)
}

fn validate_tracked_input_location(project: &Path, path: &Path) -> Result<(), StyleError> {
    for excluded in [
        ".git",
        "build",
        "target",
        ".infra/.tmp",
        ".infra/transactions",
        ".infra/.transactions",
    ] {
        if path.starts_with(project.join(excluded)) {
            return Err(StyleError::Configuration(format!(
                "active configuration input is in an untracked directory: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn globs(values: &[String]) -> Result<GlobSet, StyleError> {
    let mut builder = GlobSetBuilder::new();
    for value in values {
        if value.starts_with('/') || value.split('/').any(|part| part == "..") || value.contains('\\') {
            return Err(StyleError::Configuration(format!("unsafe scope glob: {value}")));
        }
        builder.add(Glob::new(value).map_err(|e| StyleError::Configuration(format!("invalid glob {value}: {e}")))?);
    }
    builder.build().map_err(StyleError::configuration_message)
}

fn unique(values: &[String], label: &str) -> Result<(), StyleError> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(StyleError::Configuration(format!("duplicate {label}: {value}")));
        }
    }
    Ok(())
}

pub(crate) fn load(root: &Path) -> Result<Loaded, StyleError> {
    let mut inputs = Vec::new();
    let project: Project = read_toml(root, ".infra/project.toml", &mut inputs)?;
    let config: Config = read_toml(root, ".infra/style/config.toml", &mut inputs)?;
    let policy: Policy = read_toml(root, ".infra/style/policy/rules.toml", &mut inputs)?;
    if project.schema != 1 || config.schema != 1 || policy.schema != 1 {
        return Err(StyleError::Configuration("unsupported configuration schema".into()));
    }
    if !["application", "library", "header-only"].contains(&project.kind.as_str()) {
        return Err(StyleError::Configuration(format!(
            "unknown project kind: {}",
            project.kind
        )));
    }
    for (name, profile) in &project.profiles {
        if name.trim().is_empty()
            || profile.configure.trim().is_empty()
            || profile.build.trim().is_empty()
            || profile.test.as_ref().is_some_and(|s| s.trim().is_empty())
            || profile.conan_context.as_ref().is_some_and(|s| s.trim().is_empty())
        {
            return Err(StyleError::Configuration(format!("empty profile field: {name}")));
        }
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for value in project
        .source_roots
        .iter()
        .chain(&project.test_roots)
        .chain(&project.generated_roots)
        .chain(&project.third_party_roots)
    {
        let resolved = confined(root, value)?;
        if !resolved.is_dir() {
            return Err(StyleError::Configuration(format!(
                "scope root is not a directory: {value}"
            )));
        }
        if roots
            .iter()
            .any(|p| p.starts_with(&resolved) || resolved.starts_with(p))
        {
            return Err(StyleError::Configuration(format!("overlapping project roots: {value}")));
        }
        roots.push(resolved);
    }
    unique(&config.extensions, "extension")?;
    unique(&config.rules, "rule")?;
    unique(&policy.required_rules, "required rule")?;
    if config.rules.is_empty()
        || config.extensions.is_empty()
        || config.include.is_empty()
        || config
            .extensions
            .iter()
            .any(|s| s.is_empty() || s.contains(['.', '/', '\\', '*', '?']))
    {
        return Err(StyleError::Configuration(
            "rules/extensions/include must be nonempty and extensions must be literal suffixes".into(),
        ));
    }
    for rule in config.rules.iter().chain(&policy.required_rules) {
        if !RULES.contains(&rule.as_str()) {
            return Err(StyleError::Configuration(format!("unknown rule: {rule}")));
        }
    }
    for rule in &policy.required_rules {
        if !config.rules.contains(rule) {
            return Err(StyleError::Configuration(format!(
                "organization-required rule removed: {rule}"
            )));
        }
    }
    if policy.formatter_major == 0 || policy.spdx_license.trim().is_empty() || policy.copyright.trim().is_empty() {
        return Err(StyleError::Configuration("invalid formatter/SPDX policy".into()));
    }
    let exceptions = match &config.exceptions {
        Some(path) => {
            let value: Exceptions = read_toml(root, path, &mut inputs)?;
            if value.schema != 1 {
                return Err(StyleError::Configuration("unsupported exceptions schema".into()));
            }
            value.exceptions
        }
        None => Vec::new(),
    };
    let mut seen = BTreeSet::new();
    for exception in &exceptions {
        if !RULES.contains(&exception.rule.as_str())
            || exception.rule == "format"
            || !config.rules.contains(&exception.rule)
        {
            return Err(StyleError::Configuration(format!(
                "rule cannot be exempted: {}",
                exception.rule
            )));
        }
        if exception.path.contains(['*', '?', '[', ']']) || exception.reason.trim().is_empty() {
            return Err(StyleError::Configuration(
                "exception requires an exact path and a nonempty reason".into(),
            ));
        }
        let path = confined(root, &exception.path)?;
        if !path.is_file() || !seen.insert((&exception.rule, &exception.path)) {
            return Err(StyleError::Configuration(format!(
                "invalid or duplicate exception: {}",
                exception.path
            )));
        }
    }
    let (formatter, _) = ConfigurationSnapshot::capture(root, ".infra/style/policy/clang-format.yaml")?;
    inputs.push(formatter);
    let loaded = Loaded {
        include: globs(&config.include)?,
        exclude: globs(&config.exclude)?,
        formatter_config: configuration_input(root, ".infra/style/policy/clang-format.yaml")?,
        project,
        config,
        policy,
        exceptions,
        inputs,
    };
    loaded.verify_inputs(root)?;
    Ok(loaded)
}
