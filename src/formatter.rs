// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Bounded subprocess execution using an explicit public formatter
//! configuration.

use std::fs;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use serde_json::json;
use tempfile::tempfile;
use wait_timeout::ChildExt;

use crate::report::Report;
use crate::style_error::StyleError;

pub(crate) static CANCELLED: AtomicBool = AtomicBool::new(false);

pub(crate) fn program() -> String {
    std::env::var("CPP_INFRA_CLANG_FORMAT").unwrap_or_else(|_| "clang-format".into())
}

pub(crate) fn invoke(
    project: &Path,
    args: &[String],
    input: &[u8],
    report: &mut Report,
    timeout: u64,
) -> Result<Vec<u8>, StyleError> {
    if CANCELLED.load(Ordering::Relaxed) {
        return Err(StyleError::Cancelled);
    }
    let program = program();
    let mut stdin = tempfile()?;
    stdin.write_all(input)?;
    stdin.seek(SeekFrom::Start(0))?;
    let number = report.value["commands"].as_array().map_or(0, Vec::len);
    let (stderr, log_relative) = report.create_log(number)?;
    let log = report.root.join(&log_relative);
    let stdout = tempfile()?;
    #[cfg(windows)]
    let termination_stderr = stderr.try_clone()?;
    let mut command = Command::new(&program);
    command
        .args(args)
        .current_dir(project)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let started = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|e| StyleError::Infrastructure(format!("cannot spawn {program}: {e}")))?;
    let mut cancelled = false;
    let mut timed_out = false;
    #[cfg(windows)]
    let mut tree_termination_error = None;
    let status = loop {
        if let Some(status) = child.wait_timeout(Duration::from_millis(50))? {
            break status;
        }
        cancelled = CANCELLED.load(Ordering::Relaxed);
        timed_out = started.elapsed() >= Duration::from_secs(timeout);
        if cancelled || timed_out {
            #[cfg(windows)]
            {
                let mut termination = windows_tree_termination_command(child.id());
                let termination_program = termination.get_program().to_string_lossy().into_owned();
                let termination_args: Vec<String> = termination
                    .get_args()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect();
                termination
                    .current_dir(project)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::from(termination_stderr.try_clone()?));
                let termination_started = Instant::now();
                let result = termination.status();
                report.append("commands",json!({"program":termination_program,"args":termination_args,"cwd":project,"env":{},"exit_code":result.as_ref().ok().and_then(std::process::ExitStatus::code),"duration_ms":termination_started.elapsed().as_millis(),"log":log_relative}),"invalid commands state")?;
                match result {
                    Ok(status) if status.success() => {}
                    Ok(status) => {
                        tree_termination_error = Some(StyleError::Infrastructure(format!(
                            "Windows process-tree termination failed: taskkill returned {status}"
                        )))
                    }
                    Err(error) => {
                        tree_termination_error = Some(StyleError::Infrastructure(format!(
                            "cannot terminate Windows process tree with taskkill: {error}"
                        )))
                    }
                }
            }
            #[cfg(unix)]
            {
                // The subprocess owns this process group; negative PID targets only its
                // descendants.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
            }
            let _ = child.kill();
            break child.wait()?;
        }
    };
    report.append("commands",json!({"program":program,"args":args,"cwd":project,"env":{},"exit_code":status.code(),"duration_ms":started.elapsed().as_millis(),"log":log_relative}),"invalid commands state")?;
    let stderr = fs::read(&log)?;
    report.append(
        "artifacts",
        json!({"kind":"command-log","path":log_relative,"sha256":crate::scope::hash(&stderr),"required":true}),
        "invalid artifacts state",
    )?;
    #[cfg(windows)]
    if let Some(error) = tree_termination_error {
        return Err(error);
    }
    if cancelled {
        return Err(StyleError::Cancelled);
    }
    if timed_out {
        return Err(StyleError::Infrastructure(format!(
            "{program} exceeded {timeout}s timeout"
        )));
    }
    if !status.success() {
        return Err(StyleError::Infrastructure(format!(
            "{program} failed with {status}: {}",
            String::from_utf8_lossy(&stderr)
        )));
    }
    let mut stdout = stdout;
    stdout.seek(SeekFrom::Start(0))?;
    let mut output = Vec::new();
    std::io::Read::read_to_end(&mut stdout, &mut output)?;
    Ok(output)
}

#[cfg(any(windows, test))]
fn windows_tree_termination_command(pid: u32) -> Command {
    let mut command = Command::new("taskkill");
    command.args(["/PID", &pid.to_string(), "/T", "/F"]);
    command
}

pub(crate) fn verify_version(project: &Path, major: u32, report: &mut Report, timeout: u64) -> Result<(), StyleError> {
    let bytes = invoke(project, &["--version".into()], &[], report, timeout)?;
    let text = String::from_utf8(bytes)
        .map_err(|e| StyleError::Configuration(format!("invalid clang-format version output: {e}")))?;
    report.value["external_versions"]["clang-format"] = json!(text.trim());
    report.value["data"]["formatter_major_required"] = json!(major);
    let parsed = text
        .split("version")
        .nth(1)
        .and_then(|tail| tail.split_whitespace().next())
        .and_then(|number| number.split('.').next())
        .and_then(|number| number.parse::<u32>().ok());
    if parsed != Some(major) {
        return Err(StyleError::Configuration(format!(
            "clang-format version incompatible: required major {major}, actual {}",
            text.trim()
        )));
    }
    Ok(())
}

pub(crate) fn format(
    project: &Path,
    path: &Path,
    policy: &Path,
    input: &[u8],
    report: &mut Report,
    timeout: u64,
) -> Result<Vec<u8>, StyleError> {
    let args = vec![
        format!("--style=file:{}", policy.display()),
        format!("--assume-filename={}", path.display()),
    ];
    let output = invoke(project, &args, input, report, timeout)?;
    std::str::from_utf8(&output)
        .map_err(|e| StyleError::Infrastructure(format!("formatter produced non-UTF8 output: {e}")))?;
    if !input.is_empty() && output.is_empty() {
        return Err(StyleError::Infrastructure(
            "formatter returned empty output for nonempty source".into(),
        ));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use crate::formatter::windows_tree_termination_command;

    #[test]
    fn test_windows_termination_plan_targets_the_pid_and_entire_tree() {
        let command = windows_tree_termination_command(12345);
        assert_eq!(command.get_program(), OsStr::new("taskkill"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                OsStr::new("/PID"),
                OsStr::new("12345"),
                OsStr::new("/T"),
                OsStr::new("/F")
            ]
        );
    }
}
