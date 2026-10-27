// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Command-line adaptation for the style engine.

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use clap::Parser;
use clap::Subcommand;
use serde_json::json;

use crate::config::load;
use crate::engine::execute;
use crate::engine::plan;
use crate::report::Report;
use crate::report::tool;
use crate::scope::scan;
use crate::style_error::StyleError;

#[derive(Parser)]
#[command(
    name = "cpp-infra-style",
    about = "Check C++ formatting and mechanical style rules",
    disable_version_flag = true
)]
struct Cli {
    #[arg(long, global = true, default_value = ".")]
    project: PathBuf,
    #[arg(long, global = true)]
    report: Option<PathBuf>,
    #[arg(long, global = true)]
    offline: bool,
    #[arg(long, global = true)]
    run_context: Option<PathBuf>,
    #[arg(long, global = true, default_value_t = 30)]
    timeout_seconds: u64,
    #[arg(long)]
    version: bool,
    #[arg(long, requires = "version")]
    json: bool,
    #[command(subcommand)]
    command: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    Plan,
    Check,
    Fix {
        #[arg(long)]
        dry_run: bool,
    },
}

/// Parse command-line arguments, execute the workflow, and return its exit
/// code. Filesystem and subprocess failures are translated into shared protocol
/// codes.
pub fn run() -> i32 {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return code;
        }
    };
    if cli.version {
        if cli.json {
            let mut value = tool();
            value["protocol"] = json!(1);
            println!("{value}");
        } else {
            println!("cpp-infra-style {}", env!("CARGO_PKG_VERSION"));
        }
        return 0;
    }
    let fallback = if cli.project.is_absolute() {
        cli.project.clone()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(&cli.project)
    };
    let canonical = fs::canonicalize(&cli.project);
    let root = canonical.as_ref().unwrap_or(&fallback);
    let output = cli.report.as_ref().map(|p| {
        if p.is_absolute() {
            p.clone()
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(p)
        }
    });
    let mut report = Report::new(root, output.as_deref());
    let mut output_safe = false;
    let result = (|| -> Result<bool, StyleError> {
        validate_output(root, &report.output)?;
        output_safe = !report.output.starts_with(root)
            || report.output.starts_with(root.join("build"))
            || report.output.starts_with(root.join("target"));
        let project = canonical
            .as_ref()
            .map_err(|e| StyleError::Configuration(format!("invalid project directory: {e}")))?;
        if !project.is_dir() {
            return Err(StyleError::Configuration("project must be a directory".into()));
        }
        if cli.timeout_seconds == 0 {
            return Err(StyleError::Configuration("timeout-seconds must be positive".into()));
        }
        let command = cli
            .command
            .as_ref()
            .ok_or_else(|| StyleError::Configuration("a command is required: plan, check, or fix".into()))?;
        let loaded = load(project)?;
        if let Err(error) = crate::source_exclusions::validate_file(project, &loaded, &report.output) {
            output_safe = false;
            return Err(error);
        }
        output_safe = true;
        for relative in loaded
            .project
            .source_roots
            .iter()
            .chain(&loaded.project.test_roots)
            .chain(&loaded.project.generated_roots)
            .chain(&loaded.project.third_party_roots)
        {
            if report.output.starts_with(project.join(relative)) {
                output_safe = false;
                return Err(StyleError::Configuration(
                    "report destination overlaps project source scope".into(),
                ));
            }
        }
        let sources = scan(project, &loaded)?;
        report.initialize(project, cli.run_context.as_deref(), &loaded)?;
        loaded.verify_inputs(project)?;
        if matches!(command, Action::Plan) {
            let value = plan(project, &loaded, &sources, cli.timeout_seconds);
            report.value["data"]["plan"] = value.clone();
            report.value["data"]["mode"] = json!("plan");
            println!(
                "{}",
                serde_json::to_string_pretty(&value).map_err(StyleError::infrastructure_message)?
            );
            return Ok(false);
        }
        let _ = cli.offline;
        ctrlc::set_handler(|| crate::formatter::CANCELLED.store(true, std::sync::atomic::Ordering::Relaxed))
            .map_err(|e| StyleError::Infrastructure(format!("install cancellation handler: {e}")))?;
        let (fix, dry_run) = match command {
            Action::Fix { dry_run } => (true, *dry_run),
            _ => (false, false),
        };
        report.value["data"]["mode"] = json!(if dry_run {
            "dry-run"
        } else if fix {
            "fix"
        } else {
            "check"
        });
        execute(
            project,
            &loaded,
            &sources,
            &mut report,
            fix,
            dry_run,
            cli.timeout_seconds,
        )
    })();
    let code = match &result {
        Ok(false) => 0,
        Ok(true) => 1,
        Err(error) => {
            eprintln!("{error}");
            error.exit_code()
        }
    };
    if !output_safe {
        eprintln!("report not written: output destination is unsafe");
        return code;
    }
    if let Err(error) = report.finish(result.as_ref().err(), matches!(result, Ok(true))) {
        eprintln!("cannot write report: {error}");
        return 3;
    }
    code
}

fn validate_output(project: &Path, output: &Path) -> Result<(), StyleError> {
    if output.extension().and_then(|v| v.to_str()) != Some("json")
        || output.starts_with(project.join(".infra"))
        || output
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(StyleError::Configuration(
            "report must be a normalized JSON output outside .infra".into(),
        ));
    }
    let mut ancestor = output;
    while !ancestor.exists() {
        ancestor = ancestor
            .parent()
            .ok_or_else(|| StyleError::Configuration("report output has no existing ancestor".into()))?;
    }
    if fs::canonicalize(ancestor)? != ancestor {
        return Err(StyleError::Configuration("report path contains a symbolic link".into()));
    }
    if output.exists() {
        crate::source_exclusions::existing_report(output)?;
    }
    Ok(())
}
