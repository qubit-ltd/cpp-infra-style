// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Complete check and conservative formatting workflows.

use std::fs;
use std::path::Path;

use serde_json::Value;
use serde_json::json;
use similar::TextDiff;

use crate::config::Loaded;
use crate::config::RULES;
use crate::formatter::format;
use crate::formatter::verify_version;
use crate::mechanical_rules::check;
use crate::mechanical_rules::diagnostic;
use crate::mechanical_rules::exceptions;
use crate::report::Report;
use crate::safe_write::replace;
use crate::scope::Source;
use crate::scope::scan;
use crate::style_error::StyleError;

pub(crate) fn plan(project: &Path, loaded: &Loaded, sources: &[Source], timeout: u64) -> Value {
    let mut nodes = vec![
        json!({"id":"formatter-version","tool":"clang-format","args":["--version"],"profile":null,"needs":[],"resource_locks":[],"timeout_seconds":timeout,"required":true,"output":"version compatibility"}),
    ];
    for (index, source) in sources.iter().enumerate() {
        nodes.push(json!({"id":format!("format-{index}"),"tool":"clang-format","args":[format!("--style=file:{}",loaded.formatter_config.display()),format!("--assume-filename={}",source.path.display())],"profile":null,"needs":["formatter-version"],"resource_locks":[],"timeout_seconds":timeout,"required":true,"output":source.relative}));
    }
    nodes.push(json!({"id":"mechanical-rules","tool":"cpp-infra-style","args":["check"],"profile":null,"needs":[],"resource_locks":[],"timeout_seconds":timeout,"required":true,"output":{"rules":loaded.config.rules,"files":sources.len()}}));
    json!({"schema":1,"tool":"cpp-infra-style","project":project,"nodes":nodes})
}

fn evaluate(
    project: &Path,
    loaded: &Loaded,
    sources: &[Source],
    report: &mut Report,
    timeout: u64,
) -> Result<(Vec<Value>, Vec<Vec<u8>>), StyleError> {
    let mut diagnostics = Vec::new();
    let mut formatted_files = Vec::new();
    for source in sources {
        loaded.verify_inputs(project)?;
        let formatted = if loaded.config.rules.iter().any(|rule| rule == "format") {
            format(
                project,
                &source.path,
                &loaded.formatter_config,
                &source.original,
                report,
                timeout,
            )?
        } else {
            source.original.clone()
        };
        loaded.verify_inputs(project)?;
        if formatted != source.original {
            diagnostics.push(diagnostic(
                &source.relative,
                1,
                "format",
                "run cpp-infra-style fix to apply the public formatter policy",
            ));
        }
        diagnostics.extend(check(source, loaded)?);
        formatted_files.push(formatted);
    }
    Ok((diagnostics, formatted_files))
}

pub(crate) fn execute(
    project: &Path,
    loaded: &Loaded,
    sources: &[Source],
    report: &mut Report,
    fix: bool,
    dry_run: bool,
    timeout: u64,
) -> Result<bool, StyleError> {
    loaded.verify_inputs(project)?;
    if let Ok(output) = std::process::Command::new("git")
        .args(["-C"])
        .arg(project)
        .args(["rev-parse", "HEAD"])
        .output()
        && output.status.success()
    {
        report.value["source_revision"] = json!(String::from_utf8_lossy(&output.stdout).trim());
    }
    report.value["data"]["rule_catalog"] = json!(RULES.iter().map(|rule| json!({"id":rule,"version":1,"extensions":loaded.config.extensions,"suppressible":*rule != "format"})).collect::<Vec<_>>());
    report.value["data"]["files"] = json!(sources.iter().map(|source| &source.relative).collect::<Vec<_>>());
    if loaded.config.rules.iter().any(|r| r == "format") {
        verify_version(project, loaded.policy.formatter_major, report, timeout)?;
    }
    let (mut diagnostics, formatted_files) = evaluate(project, loaded, sources, report, timeout)?;
    report.value["diagnostics"] = json!(diagnostics);
    apply_exceptions(report, &mut diagnostics, loaded)?;
    if fix {
        if !loaded.config.rules.iter().any(|r| r == "format") {
            return Err(StyleError::Configuration(
                "fix requires the format rule to be enabled".into(),
            ));
        }
        let mut changed_files = Vec::new();
        let mut patch = String::new();
        let pending: Vec<String> = sources
            .iter()
            .zip(&formatted_files)
            .filter(|(source, output)| source.original != **output)
            .map(|(source, _)| source.relative.clone())
            .collect();
        report.value["data"]["unfinished_files"] = json!(pending);
        for (source, formatted) in sources.iter().zip(&formatted_files) {
            if source.original == *formatted {
                continue;
            }
            let original_text = std::str::from_utf8(&source.original).map_err(StyleError::configuration_message)?;
            let formatted_text = std::str::from_utf8(formatted).map_err(StyleError::infrastructure_message)?;
            patch.push_str(
                &TextDiff::from_lines(original_text, formatted_text)
                    .unified_diff()
                    .header(&format!("a/{}", source.relative), &format!("b/{}", source.relative))
                    .to_string(),
            );
            if !dry_run {
                if crate::formatter::CANCELLED.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(StyleError::Cancelled);
                }
                loaded.verify_inputs(project)?;
                if loaded.is_configuration_input(&source.path) {
                    return Err(StyleError::Configuration(format!(
                        "format scope overlaps an active configuration input: {}",
                        source.relative
                    )));
                }
                replace(source, formatted)?;
                changed_files.push(source.relative.clone());
                report.value["data"]["changed_files"] = json!(changed_files);
                report.value["data"]["unfinished_files"] = json!(
                    pending
                        .iter()
                        .filter(|p| !changed_files.contains(p))
                        .collect::<Vec<_>>()
                );
            }
        }
        if !patch.is_empty() {
            report.artifact("format-diff", "style.patch", patch.as_bytes())?;
        }
        if dry_run {
            print!("{patch}");
            report.value["data"]["would_change_files"] = json!(pending);
        } else {
            loaded.verify_inputs(project)?;
            let current = scan(project, loaded)?;
            let (checked, _) = evaluate(project, loaded, &current, report, timeout)?;
            diagnostics = checked;
            report.value["diagnostics"] = json!(diagnostics);
            apply_exceptions(report, &mut diagnostics, loaded)?;
            verify_unchanged(&current)?;
            loaded.verify_inputs(project)?;
            // The report identity refers to the checked state after all successful
            // replacements.
            report.value["source_digest"] = json!(crate::scope::digest(project, false, &report.excluded)?);
        }
    }
    let failed = diagnostics.iter().any(|d| d["suppression_reason"].is_null());
    for diagnostic in &diagnostics {
        if diagnostic["suppression_reason"].is_null() {
            eprintln!(
                "{}:{}: {}: {}",
                diagnostic["project_relative_path"].as_str().unwrap_or("?"),
                diagnostic["line"],
                diagnostic["rule"].as_str().unwrap_or("?"),
                diagnostic["message"].as_str().unwrap_or("?")
            );
        }
    }
    report.value["diagnostics"] = json!(diagnostics);
    // Detect edits during a read-only check; a report must never certify old bytes.
    if !fix || dry_run {
        verify_unchanged(sources)?;
    }
    for path in &report.excluded {
        crate::source_exclusions::validate_file(project, loaded, path)?;
    }
    loaded.verify_inputs(project)?;
    if crate::scope::digest(project, true, &[])? != report.value["config_digest"] {
        return Err(StyleError::Configuration(
            "configuration changed during style execution".into(),
        ));
    }
    if crate::scope::digest(project, false, &report.excluded)? != report.value["source_digest"] {
        return Err(StyleError::ConcurrentModification(
            "project inputs changed during style execution".into(),
        ));
    }
    Ok(failed)
}

fn verify_unchanged(sources: &[Source]) -> Result<(), StyleError> {
    for source in sources {
        if fs::canonicalize(&source.path)? != source.path || fs::read(&source.path)? != source.original {
            return Err(StyleError::ConcurrentModification(source.relative.clone()));
        }
    }
    Ok(())
}

fn apply_exceptions(report: &mut Report, diagnostics: &mut [Value], loaded: &Loaded) -> Result<(), StyleError> {
    let counts = exceptions(diagnostics, loaded);
    report.value["data"]["exceptions"] = counts.clone();
    report.value["diagnostics"] = json!(diagnostics);
    if let Some(stale) = counts
        .as_array()
        .and_then(|values| values.iter().find(|entry| entry["matched"] == 0))
    {
        return Err(StyleError::Configuration(format!(
            "stale exception matches no diagnostics: {} {}",
            stale["rule"], stale["path"]
        )));
    }
    Ok(())
}
