// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

use std::fs;
use std::path::Path;
use std::process::Command;
use std::process::Output;

use serde_json::Value;
use tempfile::TempDir;
use tempfile::tempdir;

const GOOD: &str = "// SPDX-License-Identifier: Apache-2.0\n// Copyright (C) 2026 Qubit\nint main() { return 0; }\n";

fn fixture() -> TempDir {
    let dir = tempdir().expect("temporary fixture");
    fs::create_dir_all(dir.path().join(".infra/style/policy")).expect("policy directory");
    fs::create_dir_all(dir.path().join("src")).expect("source directory");
    fs::write(
        dir.path().join(".infra/project.toml"),
        "schema = 1\nkind = \"application\"\nsource_roots = [\"src\"]\n",
    )
    .expect("project config");
    fs::write(dir.path().join(".infra/style/config.toml"), "schema = 1\nextensions = [\"cpp\", \"h\"]\ninclude = [\"src/**\"]\nexclude = []\nrules = [\"format\", \"spdx-header\", \"final-newline\", \"trailing-whitespace\"]\nexceptions = \".infra/style/exceptions.toml\"\n").expect("style config");
    fs::write(
        dir.path().join(".infra/style/exceptions.toml"),
        "schema = 1\nexceptions = []\n",
    )
    .expect("exceptions");
    fs::write(dir.path().join(".infra/style/policy/rules.toml"), "schema = 1\nrequired_rules = [\"format\", \"spdx-header\", \"final-newline\", \"trailing-whitespace\"]\nformatter_major = 18\nspdx_license = \"Apache-2.0\"\ncopyright = \"Copyright (C)\"\nallow_bom = false\n").expect("rules");
    fs::write(
        dir.path().join(".infra/style/policy/clang-format.yaml"),
        "BasedOnStyle: LLVM\nIndentWidth: 2\nLineEnding: LF\n",
    )
    .expect("format policy");
    fs::write(dir.path().join("src/main.cpp"), GOOD).expect("source");
    dir
}

fn run(project: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--project")
        .arg(project)
        .arg("--report")
        .arg(project.join("build/result.json"))
        .args(args)
        .output()
        .expect("run CLI")
}
fn report(project: &Path) -> Value {
    serde_json::from_slice(&fs::read(project.join("build/result.json")).expect("report exists")).expect("valid report")
}

#[test]
fn test_check_passes_and_writes_complete_public_report() {
    let dir = fixture();
    let output = run(dir.path(), &["check"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = report(dir.path());
    for key in [
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
    ] {
        assert!(value.get(key).is_some(), "missing {key}");
    }
    assert_eq!(value["status"], "passed");
    assert_eq!(
        value["external_versions"]["clang-format"]
            .as_str()
            .map(|v| v.contains("18.")),
        Some(true)
    );
    assert!(value["commands"].as_array().expect("commands").len() >= 2);
}

#[test]
fn test_check_reads_untracked_source_without_modification() {
    let dir = fixture();
    let bad = "int  f(){return 1;}  ";
    fs::write(dir.path().join("src/new.cpp"), bad).expect("new source");
    let output = run(dir.path(), &["check"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fs::read_to_string(dir.path().join("src/new.cpp")).expect("source"), bad);
    assert!(
        report(dir.path())["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|d| d["project_relative_path"] == "src/new.cpp" && d["rule"] == "format")
    );
}

#[test]
fn test_fix_dry_run_and_idempotence() {
    let dir = fixture();
    let bad = GOOD.replace("int main() { return 0; }", "int  main(){return 0;}");
    fs::write(dir.path().join("src/main.cpp"), &bad).expect("source");
    let dry = run(dir.path(), &["fix", "--dry-run"]);
    assert_eq!(dry.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&dry.stdout).contains("src/main.cpp"));
    assert_eq!(
        fs::read_to_string(dir.path().join("src/main.cpp")).expect("source"),
        bad
    );
    assert_eq!(run(dir.path(), &["fix"]).status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(dir.path().join("src/main.cpp")).expect("fixed source"),
        GOOD
    );
    assert_eq!(run(dir.path(), &["fix"]).status.code(), Some(0));
    assert!(
        report(dir.path())["data"]["changed_files"]
            .as_array()
            .expect("changed files")
            .is_empty()
    );
}

#[test]
fn test_fix_still_fails_nonformatter_gate() {
    let dir = fixture();
    fs::write(dir.path().join("src/main.cpp"), "int  main(){return 0;}\n").expect("source");
    assert_eq!(run(dir.path(), &["fix"]).status.code(), Some(1));
    assert!(
        fs::read_to_string(dir.path().join("src/main.cpp"))
            .expect("source")
            .contains("int main()")
    );
    assert!(
        report(dir.path())["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|d| d["rule"] == "spdx-header")
    );
}

#[test]
fn test_nested_formatter_does_not_override_public_policy() {
    let dir = fixture();
    fs::write(dir.path().join("src/.clang-format"), "garbage formatter configuration").expect("nested config");
    assert_eq!(run(dir.path(), &["check"]).status.code(), Some(0));
}

#[test]
fn test_plan_never_invokes_formatter() {
    let dir = fixture();
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--project")
        .arg(dir.path())
        .arg("plan")
        .env("PATH", "")
        .output()
        .expect("plan");
    assert_eq!(output.status.code(), Some(0));
    let plan: Value = serde_json::from_slice(&output.stdout).expect("JSON plan");
    assert_eq!(plan["schema"], 1);
    assert!(!plan["nodes"].as_array().expect("nodes").is_empty());
}

#[test]
fn test_strict_configuration_and_nonempty_scope() {
    for (path, contents) in [
        (".infra/style/config.toml", "schema = 1\ntypo = true\n"),
        (
            ".infra/project.toml",
            "schema = 1\nkind = \"application\"\nsource_roots = [\"src\", \"src\"]\n",
        ),
    ] {
        let dir = fixture();
        fs::write(dir.path().join(path), contents).expect("invalid config");
        assert_eq!(run(dir.path(), &["check"]).status.code(), Some(2));
        assert_eq!(report(dir.path())["status"], "error");
    }
    let dir = fixture();
    fs::write(dir.path().join(".infra/style/config.toml"), "schema = 1\nextensions = [\"hpp\"]\ninclude = [\"src/**\"]\nrules = [\"format\", \"spdx-header\", \"final-newline\", \"trailing-whitespace\"]\n").expect("empty config");
    assert_eq!(run(dir.path(), &["check"]).status.code(), Some(2));
}

#[test]
fn test_policy_cannot_be_weakened() {
    let dir = fixture();
    let path = dir.path().join(".infra/style/config.toml");
    let text = fs::read_to_string(&path).expect("config").replace(
        "\"format\", \"spdx-header\", \"final-newline\", \"trailing-whitespace\"",
        "\"format\"",
    );
    fs::write(path, text).expect("weak config");
    assert_eq!(run(dir.path(), &["check"]).status.code(), Some(2));
}

#[test]
fn test_exception_exact_match_is_reported_and_stale_is_error() {
    let dir = fixture();
    fs::write(dir.path().join("src/main.cpp"), "int main() { return 0; }\n").expect("source");
    let path = dir.path().join(".infra/style/exceptions.toml");
    fs::write(&path, "schema = 1\n[[exceptions]]\nrule = \"spdx-header\"\npath = \"src/main.cpp\"\nreason = \"Original license retained\"\n").expect("exception");
    assert_eq!(run(dir.path(), &["check"]).status.code(), Some(0));
    assert_eq!(
        report(dir.path())["diagnostics"][0]["suppression_reason"],
        "Original license retained"
    );
    fs::write(dir.path().join("src/main.cpp"), GOOD).expect("source restored");
    assert_eq!(run(dir.path(), &["check"]).status.code(), Some(2));
    assert_eq!(report(dir.path())["data"]["exceptions"][0]["matched"], 0);
}

#[test]
fn test_rejects_wildcard_unknown_and_unexemptable_rules() {
    for (rule, path) in [
        ("spdx-header", "src/*.cpp"),
        ("unknown", "src/main.cpp"),
        ("format", "src/main.cpp"),
    ] {
        let dir = fixture();
        fs::write(
            dir.path().join(".infra/style/exceptions.toml"),
            format!("schema = 1\n[[exceptions]]\nrule = {rule:?}\npath = {path:?}\nreason = \"retained\"\n"),
        )
        .expect("exception");
        assert_eq!(run(dir.path(), &["check"]).status.code(), Some(2));
    }
}

#[test]
fn test_non_utf8_source_is_configuration_error() {
    let dir = fixture();
    fs::write(dir.path().join("src/main.cpp"), [0xff, 0xfe]).expect("non UTF8");
    assert_eq!(run(dir.path(), &["check"]).status.code(), Some(2));
}

#[test]
fn test_version_reports_unpublished_revision_as_null() {
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .args(["--version", "--json"])
        .output()
        .expect("version");
    assert_eq!(output.status.code(), Some(0));
    let value: Value = serde_json::from_slice(&output.stdout).expect("version JSON");
    assert_eq!(value["name"], "cpp-infra-style");
    assert_eq!(value["protocol"], 1);
    assert!(value["revision"].is_null());
}

#[cfg(unix)]
#[test]
fn test_symlink_escape_rejected_and_third_party_unchanged() {
    use std::os::unix::fs::symlink;
    let dir = fixture();
    let outside = tempdir().expect("outside");
    fs::write(outside.path().join("outside.cpp"), GOOD).expect("outside source");
    symlink(outside.path().join("outside.cpp"), dir.path().join("src/escape.cpp")).expect("symlink");
    assert_eq!(run(dir.path(), &["check"]).status.code(), Some(2));
    let dir = fixture();
    fs::create_dir(dir.path().join("vendor")).expect("vendor");
    fs::write(dir.path().join("vendor/bad.cpp"), "bad  ").expect("vendor source");
    let path = dir.path().join(".infra/project.toml");
    fs::write(
        path,
        "schema = 1\nkind = \"application\"\nsource_roots = [\"src\"]\nthird_party_roots = [\"vendor\"]\n",
    )
    .expect("project");
    assert_eq!(run(dir.path(), &["fix"]).status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(dir.path().join("vendor/bad.cpp")).expect("vendor"),
        "bad  "
    );
}

#[test]
fn test_unsafe_report_path_never_overwrites_source_or_configuration() {
    let dir = fixture();
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--project")
        .arg(dir.path())
        .arg("--report")
        .arg(dir.path().join("src/main.cpp"))
        .arg("check")
        .output()
        .expect("CLI");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(dir.path().join("src/main.cpp")).expect("source"),
        GOOD
    );
}

#[test]
fn test_empty_rules_cannot_certify_a_project() {
    let dir = fixture();
    for path in [".infra/style/config.toml", ".infra/style/policy/rules.toml"] {
        let target = dir.path().join(path);
        let content = fs::read_to_string(&target).expect("config").replace(
            "[\"format\", \"spdx-header\", \"final-newline\", \"trailing-whitespace\"]",
            "[]",
        );
        fs::write(target, content).expect("empty rules");
    }
    assert_eq!(run(dir.path(), &["check"]).status.code(), Some(2));
}

#[cfg(unix)]
fn fake_formatter(dir: &Path, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("formatter.py");
    fs::write(
        &path,
        format!("#!/usr/bin/python3\nimport sys, pathlib, time\n{body}\n"),
    )
    .expect("formatter fixture");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
    path
}

#[cfg(unix)]
#[test]
fn test_formatter_version_missing_crash_and_timeout_are_not_style_violations() {
    for (body, expected, marker) in [
        ("print('clang-format version 19.0.0')", 2, "incompatible"),
        (
            "if '--version' in sys.argv: print('clang-format version 18.1.3')\nelse: sys.exit(11)",
            3,
            "failed",
        ),
        ("time.sleep(2)", 3, "timeout"),
    ] {
        let dir = fixture();
        let fake = fake_formatter(dir.path(), body);
        let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
            .arg("--project")
            .arg(dir.path())
            .arg("--report")
            .arg(dir.path().join("build/result.json"))
            .args(["--timeout-seconds", "1", "check"])
            .env("CPP_INFRA_CLANG_FORMAT", fake)
            .output()
            .expect("CLI");
        assert_eq!(output.status.code(), Some(expected));
        assert!(String::from_utf8_lossy(&output.stderr).contains(marker));
        assert_eq!(report(dir.path())["status"], "error");
    }
    let dir = fixture();
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--project")
        .arg(dir.path())
        .arg("check")
        .env("CPP_INFRA_CLANG_FORMAT", dir.path().join("missing-format"))
        .output()
        .expect("CLI");
    assert_eq!(output.status.code(), Some(3));
}

#[cfg(unix)]
#[test]
fn test_concurrent_edit_is_preserved_and_permissions_survive_fix() {
    use std::os::unix::fs::PermissionsExt;
    let dir = fixture();
    let fake = fake_formatter(
        dir.path(),
        "if '--version' in sys.argv: print('clang-format version 18.1.3')\nelse:\n    path = pathlib.Path(next(a.split('=',1)[1] for a in sys.argv if a.startswith('--assume-filename=')))\n    path.write_text('new user edit\\n')\n    print('formatted output')",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--project")
        .arg(dir.path())
        .arg("--report")
        .arg(dir.path().join("build/result.json"))
        .arg("fix")
        .env("CPP_INFRA_CLANG_FORMAT", fake)
        .output()
        .expect("CLI");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(dir.path().join("src/main.cpp")).expect("user edit"),
        "new user edit\n"
    );
    assert_eq!(report(dir.path())["data"]["unfinished_files"][0], "src/main.cpp");
    let dir = fixture();
    let path = dir.path().join("src/main.cpp");
    fs::write(
        &path,
        GOOD.replace("int main() { return 0; }", "int  main(){return 0;}"),
    )
    .expect("source");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).expect("permissions");
    assert_eq!(run(dir.path(), &["fix"]).status.code(), Some(0));
    assert_eq!(
        fs::metadata(path).expect("source metadata").permissions().mode() & 0o777,
        0o640
    );
}

#[test]
fn test_context_rejects_source_change_and_accepts_current_runtime_bundle() {
    let dir = fixture();
    assert_eq!(run(dir.path(), &["check"]).status.code(), Some(0));
    let prior = report(dir.path());
    let context = serde_json::json!({"schema":1,"run_id":"shared-run","task_id":"style","bundle_id":"development","project":fs::canonicalize(dir.path()).expect("canonical project"),"profile":null,"config_digest":prior["config_digest"],"source_digest":prior["source_digest"],"report_root":dir.path().join("build/infra-runs/shared-run")});
    let context_path = dir.path().join("build/context.json");
    fs::write(&context_path, serde_json::to_vec(&context).expect("context")).expect("context file");
    let context_arg = context_path.to_str().expect("UTF8");
    assert_eq!(
        run(dir.path(), &["--run-context", context_arg, "check"]).status.code(),
        Some(0)
    );
    fs::write(dir.path().join("src/main.cpp"), GOOD.replace("return 0", "return 1")).expect("edit");
    assert_eq!(
        run(dir.path(), &["--run-context", context_arg, "check"]).status.code(),
        Some(2)
    );
    assert!(
        report(dir.path())["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|d| d["message"].as_str().is_some_and(|s| s.contains("stale")))
    );
    let mut context = context;
    context["bundle_id"] = serde_json::json!("release-1");
    fs::write(dir.path().join("src/main.cpp"), GOOD).expect("restore source");
    fs::write(&context_path, serde_json::to_vec(&context).expect("context")).expect("context file");
    assert_eq!(
        run(dir.path(), &["--run-context", context_arg, "check"]).status.code(),
        Some(0)
    );
    assert_eq!(report(dir.path())["bundle_id"], "release-1");
}

#[test]
fn test_report_cannot_overwrite_selected_json_source() {
    let dir = fixture();
    let config = dir.path().join(".infra/style/config.toml");
    let content = fs::read_to_string(&config)
        .expect("config")
        .replace("[\"cpp\", \"h\"]", "[\"cpp\", \"json\"]");
    fs::write(config, content).expect("config");
    let source = dir.path().join("src/input.json");
    fs::write(&source, GOOD).expect("source");
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--project")
        .arg(dir.path())
        .arg("--report")
        .arg(&source)
        .arg("check")
        .output()
        .expect("CLI");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(fs::read_to_string(source).expect("source"), GOOD);
}

#[cfg(unix)]
#[test]
fn test_fix_rejects_parent_symlink_substitution() {
    let dir = fixture();
    let outside = tempdir().expect("outside");
    fs::write(outside.path().join("main.cpp"), GOOD).expect("outside source");
    let fake = fake_formatter(
        dir.path(),
        &format!(
            "if '--version' in sys.argv: print('clang-format version 18.1.3')\nelse:\n    path = pathlib.Path(next(a.split('=',1)[1] for a in sys.argv if a.startswith('--assume-filename=')))\n    path.parent.rename(path.parent.with_name('saved-src'))\n    path.parent.symlink_to({:?}, target_is_directory=True)\n    print('formatted output')",
            outside.path().to_str().expect("UTF8")
        ),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--project")
        .arg(dir.path())
        .arg("--report")
        .arg(dir.path().join("build/result.json"))
        .arg("fix")
        .env("CPP_INFRA_CLANG_FORMAT", fake)
        .output()
        .expect("CLI");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(outside.path().join("main.cpp")).expect("outside source"),
        GOOD
    );
}

#[cfg(unix)]
#[test]
fn test_cancellation_terminates_formatter_process_group() {
    use std::thread::sleep;
    use std::time::Duration;
    use std::time::Instant;
    let dir = fixture();
    let marker = dir.path().join("build/started");
    let fake = fake_formatter(
        dir.path(),
        "if '--version' in sys.argv: print('clang-format version 18.1.3')\nelse:\n    import subprocess\n    proc = subprocess.Popen(['/usr/bin/python3','-c','import time; time.sleep(60)'])\n    pathlib.Path('build/started').write_text(str(proc.pid))\n    time.sleep(60)",
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--project")
        .arg(dir.path())
        .arg("--report")
        .arg(dir.path().join("build/result.json"))
        .arg("check")
        .env("CPP_INFRA_CLANG_FORMAT", fake)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("CLI");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() && Instant::now() < deadline {
        sleep(Duration::from_millis(10));
    }
    assert!(marker.exists(), "formatter started");
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    assert_eq!(child.wait().expect("cancelled CLI").code(), Some(130));
    assert_eq!(report(dir.path())["status"], "cancelled");
    let pid = fs::read_to_string(marker).expect("child PID");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => {
                if stat.split_whitespace().nth(2) == Some("Z") {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "formatter descendant must have terminated: {stat}"
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => panic!("cannot read formatter descendant status: {error}"),
        }
        sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn test_configuration_change_during_check_invalidates_result() {
    let dir = fixture();
    let fake = fake_formatter(
        dir.path(),
        "if '--version' in sys.argv: print('clang-format version 18.1.3')\nelse:\n    p = pathlib.Path('.infra/style/policy/rules.toml')\n    p.write_text(p.read_text().replace('Copyright (C)', 'Changed copyright'))\n    sys.stdout.write(sys.stdin.read())",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--project")
        .arg(dir.path())
        .arg("--report")
        .arg(dir.path().join("build/result.json"))
        .arg("check")
        .env("CPP_INFRA_CLANG_FORMAT", fake)
        .output()
        .expect("CLI");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        report(dir.path())["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|d| d["message"]
                .as_str()
                .is_some_and(|m| m.contains("configuration changed")))
    );
}

#[test]
fn test_runtime_snapshot_validates_content_bundle_tool_and_executable_identity() {
    use sha2::Digest;
    use sha2::Sha256;
    let dir = fixture();
    assert_eq!(run(dir.path(), &["check"]).status.code(), Some(0));
    let prior = report(dir.path());
    let binary = fs::canonicalize(env!("CARGO_BIN_EXE_cpp-infra-style")).expect("binary");
    let executable_sha256 = format!("{:x}", Sha256::digest(fs::read(&binary).expect("binary bytes")));
    let snapshot = serde_json::json!({"schema":1,"bundle_id":"main-current","tools":[{"name":"cpp-infra-style","version":"0.1.0","revision":null,"protocol":1,"binary":binary,"executable_sha256":executable_sha256}],"offline":true,"stale":false});
    let snapshot_path = dir.path().join("build/runtime-snapshot.json");
    let context_path = dir.path().join("build/context.json");
    let context_arg = context_path.to_str().expect("context path");
    let mut context = serde_json::json!({"schema":1,"run_id":"runtime-test","task_id":"style","bundle_id":"main-current","project":fs::canonicalize(dir.path()).expect("canonical project"),"profile":null,"source_digest":prior["source_digest"],"config_digest":prior["config_digest"],"report_root":dir.path().join("build/infra-runs/runtime-test"),"runtime_snapshot":snapshot_path,"runtime_snapshot_digest":"pending"});
    for mutation in [
        "valid",
        "file-hash",
        "bundle",
        "tool-version",
        "executable",
        "missing-tool",
    ] {
        let mut candidate = snapshot.clone();
        match mutation {
            "bundle" => candidate["bundle_id"] = serde_json::json!("other"),
            "tool-version" => candidate["tools"][0]["version"] = serde_json::json!("99.0.0"),
            "executable" => candidate["tools"][0]["executable_sha256"] = serde_json::json!("0".repeat(64)),
            "missing-tool" => candidate["tools"] = serde_json::json!([]),
            _ => {}
        }
        let bytes = serde_json::to_vec(&candidate).expect("snapshot bytes");
        fs::write(&snapshot_path, &bytes).expect("snapshot");
        context["runtime_snapshot_digest"] = serde_json::json!(if mutation == "file-hash" {
            "0".repeat(64)
        } else {
            format!("{:x}", Sha256::digest(&bytes))
        });
        fs::write(&context_path, serde_json::to_vec(&context).expect("context bytes")).expect("context");
        let result = run(dir.path(), &["--run-context", context_arg, "check"]);
        assert_eq!(
            result.status.code(),
            Some(if mutation == "valid" { 0 } else { 2 }),
            "{mutation}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn test_bom_acceptance_follows_the_public_rule_policy() {
    let dir = fixture();
    fs::write(dir.path().join("src/main.cpp"), format!("\u{feff}{GOOD}")).expect("BOM source");
    assert_eq!(run(dir.path(), &["check"]).status.code(), Some(1));
    let path = dir.path().join(".infra/style/policy/rules.toml");
    let policy = fs::read_to_string(&path)
        .expect("policy")
        .replace("allow_bom = false", "allow_bom = true");
    fs::write(path, policy).expect("allow BOM");
    assert_eq!(run(dir.path(), &["check"]).status.code(), Some(0));
}

#[test]
fn test_shipped_policy_accepts_the_team_standard_copyright_header() {
    let dir = fixture();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("conf/policy/rules.toml"),
        dir.path().join(".infra/style/policy/rules.toml"),
    )
    .expect("shipped rules");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("conf/policy/clang-format.yaml"),
        dir.path().join(".infra/style/policy/clang-format.yaml"),
    )
    .expect("shipped formatting");
    fs::write(
        dir.path().join("src/main.cpp"),
        "// SPDX-License-Identifier: Apache-2.0\n//    Copyright (c) 2026 Haixing Hu.\nint main() { return 0; }\n",
    )
    .expect("standard copyright header");
    let output = run(dir.path(), &["check"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_windows_cancellation_branch_terminates_the_process_tree() {
    let implementation = include_str!("../src/formatter.rs");
    assert!(
        implementation.contains("#[cfg(windows)]"),
        "Windows requires a dedicated process-tree cancellation branch"
    );
    assert!(
        implementation.contains("windows_tree_termination_command(child.id())"),
        "Windows cancellation must execute the tree termination plan for the running child"
    );
}
