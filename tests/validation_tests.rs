// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! User-input and infrastructure failures must produce precise, nonmutating
//! errors.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::process::Output;

use qubit_cpp_infra_style::StyleError;
use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use tempfile::TempDir;
use tempfile::tempdir;

const GOOD: &str = "// SPDX-License-Identifier: Apache-2.0\n// Copyright (c) 2026 Qubit\nint main() { return 0; }\n";

fn fixture() -> TempDir {
    let directory = tempdir().expect("temporary project");
    let project = directory.path();
    fs::create_dir_all(project.join(".infra/style/policy")).expect("policy directory");
    fs::create_dir(project.join("src")).expect("source directory");
    for file in ["rules.toml", "clang-format.yaml"] {
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("conf/policy").join(file),
            project.join(".infra/style/policy").join(file),
        )
        .expect("policy");
    }
    fs::write(
        project.join(".infra/project.toml"),
        "schema=1\nkind=\"application\"\nsource_roots=[\"src\"]\n",
    )
    .expect("project configuration");
    fs::write(project.join(".infra/style/config.toml"),"schema=1\nextensions=[\"cpp\"]\ninclude=[\"src/**\"]\nrules=[\"format\",\"spdx-header\",\"final-newline\",\"trailing-whitespace\"]\nexceptions=\".infra/style/exceptions.toml\"\n").expect("style configuration");
    fs::write(
        project.join(".infra/style/exceptions.toml"),
        "schema=1\nexceptions=[]\n",
    )
    .expect("exceptions");
    fs::write(project.join("src/main.cpp"), GOOD).expect("source");
    directory
}

fn command(project: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"));
    command
        .arg("--project")
        .arg(project)
        .arg("--report")
        .arg(project.join("build/result.json"));
    command
}

fn check(project: &Path) -> Output {
    command(project).arg("check").output().expect("CLI")
}

fn report(project: &Path) -> Value {
    serde_json::from_slice(&fs::read(project.join("build/result.json")).expect("report")).expect("report JSON")
}

fn assert_configuration_failure(project: &Path, output: &Output, fragment: &str) {
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(fragment),
        "missing {fragment}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(report(project)["status"], "error");
    assert_eq!(fs::read_to_string(project.join("src/main.cpp")).expect("source"), GOOD);
}

#[test]
fn test_every_toml_input_rejects_encoding_and_schema_errors_without_source_changes() {
    for relative in [
        ".infra/project.toml",
        ".infra/style/config.toml",
        ".infra/style/policy/rules.toml",
        ".infra/style/exceptions.toml",
    ] {
        for bytes in [
            vec![0xff, 0xfe],
            b"schema=1\nunknown_field=true\n".to_vec(),
            b"schema=2\n".to_vec(),
        ] {
            let directory = fixture();
            let project = directory.path();
            let schema_only = bytes == b"schema=2\n";
            let bytes = if schema_only {
                fs::read_to_string(project.join(relative))
                    .expect("valid configuration")
                    .replace("schema=1", "schema=2")
                    .replace("schema = 1", "schema = 2")
                    .into_bytes()
            } else {
                bytes
            };
            fs::write(project.join(relative), &bytes).expect("invalid input");
            let output = check(project);
            assert_configuration_failure(
                project,
                &output,
                if bytes[0] == 0xff {
                    "non-UTF8 configuration"
                } else if schema_only {
                    "unsupported"
                } else {
                    "invalid"
                },
            );
            assert_eq!(fs::read(project.join(relative)).expect("configuration"), bytes);
        }
    }
}

#[test]
fn test_profile_optional_fields_and_bad_project_roots_are_validated() {
    let directory = fixture();
    let project = directory.path();
    let base = fs::read_to_string(project.join(".infra/project.toml")).expect("project configuration");
    for (test, conan, expected) in [("debug", "linux-debug", 0), ("", "linux-debug", 2), ("debug", "", 2)] {
        fs::write(project.join(".infra/project.toml"),format!("{base}\n[profiles.debug]\nconfigure=\"debug\"\nbuild=\"debug\"\ntest={test:?}\nconan_context={conan:?}\n")).expect("profile");
        assert_eq!(check(project).status.code(), Some(expected));
    }
    for (roots, fragment) in [
        ("[\"missing\"]", "invalid project path"),
        ("[\"not-a-directory\"]", "not a directory"),
        ("[\"../outside\"]", "project-relative"),
        ("[\"src\",\"src/nested\"]", "overlapping"),
    ] {
        fs::write(project.join("not-a-directory"), "ordinary input").expect("ordinary input");
        fs::create_dir_all(project.join("src/nested")).expect("nested directory");
        fs::write(
            project.join(".infra/project.toml"),
            format!("schema=1\nkind=\"application\"\nsource_roots={roots}\n"),
        )
        .expect("bad roots");
        assert_configuration_failure(project, &check(project), fragment);
    }
}

#[test]
fn test_invalid_globs_and_rule_policy_are_diagnosed_before_formatting() {
    for (include, fragment) in [
        ("[", "invalid glob"),
        ("../src/**", "unsafe scope glob"),
        ("/src/**", "unsafe scope glob"),
        ("src\\**", "unsafe scope glob"),
    ] {
        let directory = fixture();
        let project = directory.path();
        let config = project.join(".infra/style/config.toml");
        fs::write(
            &config,
            fs::read_to_string(&config)
                .expect("configuration")
                .replace("\"src/**\"", &format!("{include:?}")),
        )
        .expect("bad glob");
        assert_configuration_failure(project, &check(project), fragment);
        assert!(report(project)["commands"].as_array().expect("commands").is_empty());
    }
    for (relative, before, after, fragment) in [
        (".infra/project.toml", "application", "unknown", "unknown project kind"),
        (
            ".infra/style/config.toml",
            "extensions=[\"cpp\"]",
            "extensions=[\".cpp\"]",
            "literal suffixes",
        ),
        (
            ".infra/style/config.toml",
            "\"format\",",
            "\"unknown-rule\",",
            "unknown rule",
        ),
        (
            ".infra/style/policy/rules.toml",
            "formatter_major = 18",
            "formatter_major = 0",
            "invalid formatter/SPDX",
        ),
        (
            ".infra/style/exceptions.toml",
            "schema=1",
            "schema=2",
            "unsupported exceptions schema",
        ),
    ] {
        let directory = fixture();
        let project = directory.path();
        let path = project.join(relative);
        fs::write(
            &path,
            fs::read_to_string(&path).expect("configuration").replace(before, after),
        )
        .expect("bad configuration");
        assert_configuration_failure(project, &check(project), fragment);
    }
}

#[test]
fn test_mechanical_only_policy_does_not_require_formatter_and_refuses_fix() {
    let directory = fixture();
    let project = directory.path();
    for (relative, before, after) in [
        (
            ".infra/style/config.toml",
            "rules=[\"format\",\"spdx-header\",\"final-newline\",\"trailing-whitespace\"]",
            "rules=[\"spdx-header\"]",
        ),
        (
            ".infra/style/policy/rules.toml",
            "required_rules = [\"format\", \"spdx-header\", \"final-newline\", \"trailing-whitespace\"]",
            "required_rules = [\"spdx-header\"]",
        ),
    ] {
        let path = project.join(relative);
        fs::write(
            &path,
            fs::read_to_string(&path).expect("configuration").replace(before, after),
        )
        .expect("mechanical policy");
    }
    let checked = command(project)
        .arg("check")
        .env("CPP_INFRA_CLANG_FORMAT", project.join("absent-formatter"))
        .output()
        .expect("check");
    assert_eq!(checked.status.code(), Some(0));
    assert!(report(project)["commands"].as_array().expect("commands").is_empty());
    assert_configuration_failure(
        project,
        &command(project).arg("fix").output().expect("fix"),
        "fix requires the format rule",
    );
}

#[test]
fn test_cli_help_versions_and_missing_selection_have_stable_exit_codes() {
    let help = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--help")
        .output()
        .expect("help");
    assert_eq!(help.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&help.stdout).contains("fix"));
    let version = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--version")
        .output()
        .expect("version");
    assert_eq!(version.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&version.stdout).trim(), "cpp-infra-style 0.1.0");
    let directory = fixture();
    let project = directory.path();
    assert_configuration_failure(
        project,
        &command(project).output().expect("missing command"),
        "a command is required",
    );
    assert_configuration_failure(
        project,
        &command(project)
            .args(["--timeout-seconds", "0", "check"])
            .output()
            .expect("timeout"),
        "timeout-seconds must be positive",
    );
    let outside = tempdir().expect("external report directory");
    let invalid = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--project")
        .arg(project.join("absent-project"))
        .arg("--report")
        .arg(outside.path().join("error.json"))
        .arg("check")
        .output()
        .expect("invalid project");
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("invalid project directory"));
}

#[test]
fn test_io_error_conversion_preserves_context_and_infrastructure_exit_class() {
    let error = StyleError::from(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "input volume is unreadable",
    ));
    assert!(matches!(error, StyleError::Infrastructure(_)));
    assert_eq!(error.exit_code(), 3);
    assert_eq!(error.to_string(), "input volume is unreadable");
}

#[test]
fn test_project_owned_delivery_template_is_manifested_and_runs_with_actual_scope() {
    let manifest: Value = serde_json::from_slice(
        &fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("conf/manifest.json")).expect("manifest"),
    )
    .expect("manifest JSON");
    let templates = manifest["project_files"]
        .as_array()
        .expect("project-owned init templates");
    let entry = templates
        .iter()
        .find(|entry| entry["target"] == ".infra/style/config.toml")
        .expect("style init configuration");
    assert!(
        !manifest["files"]
            .as_array()
            .expect("managed files")
            .iter()
            .any(|managed| managed["target"] == entry["target"]),
        "project-owned configuration must not become an update-managed policy"
    );
    let template = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("conf")
            .join(entry["source"].as_str().expect("template source")),
    )
    .expect("template bytes");
    assert_eq!(entry["sha256"], format!("{:x}", Sha256::digest(&template)));
    let directory = fixture();
    let project = directory.path();
    fs::write(project.join(".infra/style/config.toml"), template).expect("install shipped init template");
    fs::create_dir(project.join("actual_sources")).expect("actual project source scope");
    fs::rename(project.join("src/main.cpp"), project.join("actual_sources/main.cpp"))
        .expect("move disposable fixture source");
    fs::write(
        project.join(".infra/project.toml"),
        "schema=1\nkind=\"application\"\nsource_roots=[\"actual_sources\"]\n",
    )
    .expect("actual source ownership");
    let output = check(project);
    assert_eq!(
        output.status.code(),
        Some(0),
        "shipped template must be immediately usable: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(report(project)["data"]["files"], json!(["actual_sources/main.cpp"]));
}

#[cfg(unix)]
fn wrapper(directory: &Path, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = directory.join("formatter.py");
    fs::write(&path, format!("#!/usr/bin/python3\nimport sys,os,pathlib\n{body}\n")).expect("formatter");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable formatter");
    path
}

#[cfg(unix)]
#[test]
fn test_formatter_protocol_rejects_non_utf8_versions_outputs_and_empty_success() {
    for (body, expected, fragment) in [
        ("os.write(1,b'\\xff')", 2, "invalid clang-format version output"),
        (
            "if '--version' in sys.argv: print('clang-format version 18.1.3')\nelse: os.write(1,b'\\xff')",
            3,
            "formatter produced non-UTF8 output",
        ),
        (
            "if '--version' in sys.argv: print('clang-format version 18.1.3')",
            3,
            "empty output",
        ),
        ("print('not a version')", 2, "version incompatible"),
    ] {
        let directory = fixture();
        let project = directory.path();
        let temporary = tempdir().expect("formatter directory");
        let executable = wrapper(temporary.path(), body);
        let output = command(project)
            .arg("check")
            .env("CPP_INFRA_CLANG_FORMAT", executable)
            .output()
            .expect("check");
        assert_eq!(
            output.status.code(),
            Some(expected),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains(fragment));
        assert_eq!(report(project)["status"], "error");
        assert_eq!(fs::read_to_string(project.join("src/main.cpp")).expect("source"), GOOD);
    }
}

#[cfg(unix)]
#[test]
fn test_non_utf8_filesystem_names_are_rejected_in_source_and_digest_inputs() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    for source in [true, false] {
        let directory = fixture();
        let project = directory.path();
        let name = OsString::from_vec(vec![b'n', 0xff, b'.', b'c', b'p', b'p']);
        let path = if source {
            project.join("src").join(name)
        } else {
            project.join(name)
        };
        fs::write(&path, GOOD).expect("non-UTF8 name");
        assert_configuration_failure(project, &check(project), "non-UTF8 project path");
        assert_eq!(fs::read(path).expect("unmodified input"), GOOD.as_bytes());
    }
}

#[test]
fn test_malformed_run_context_and_runtime_snapshot_fields_are_precise_errors() {
    let directory = fixture();
    let project = directory.path();
    assert_eq!(check(project).status.code(), Some(0));
    let baseline = report(project);
    let context_path = project.join("build/context.json");
    let snapshot_path = project.join("build/snapshot.json");
    let binary = fs::canonicalize(env!("CARGO_BIN_EXE_cpp-infra-style")).expect("binary");
    let mut context = json!({"schema":1,"run_id":"validation","task_id":"style","bundle_id":"validation-bundle","project":project,"profile":null,"source_digest":baseline["source_digest"],"config_digest":baseline["config_digest"],"report_root":project.join("build/infra-runs/validation")});
    let snapshot = json!({"schema":1,"bundle_id":"validation-bundle","tools":[{"name":"cpp-infra-style","version":"0.1.0","revision":null,"protocol":1,"binary":binary,"executable_sha256":format!("{:x}",Sha256::digest(fs::read(&binary).expect("binary bytes")))}],"offline":true,"stale":false});
    fs::write(&context_path, b"not JSON").expect("malformed context");
    assert_configuration_failure(
        project,
        &command(project)
            .arg("--run-context")
            .arg(&context_path)
            .arg("check")
            .output()
            .expect("context"),
        "invalid run context",
    );
    for (field, value, fragment) in [
        ("schema", json!(2), "identity mismatch"),
        ("run_id", json!(""), "identity mismatch"),
        ("project", json!(project.join("other")), "identity mismatch"),
        (
            "report_root",
            json!(project.join("outside")),
            "outside build/infra-runs",
        ),
    ] {
        let mut invalid = context.clone();
        invalid[field] = value;
        fs::write(&context_path, serde_json::to_vec(&invalid).expect("JSON")).expect("context");
        assert_configuration_failure(
            project,
            &command(project)
                .arg("--run-context")
                .arg(&context_path)
                .arg("check")
                .output()
                .expect("context"),
            fragment,
        );
    }
    context["runtime_snapshot"] = json!(snapshot_path);
    for (mutation, fragment) in [
        ("missing-pair", "appear together"),
        ("malformed", "invalid runtime snapshot"),
        ("tools", "tools missing"),
        ("binary", "binary path missing"),
        ("digest", "executable digest missing"),
        ("relative-binary", "existing absolute file"),
        ("different-executable", "executable digest mismatch"),
    ] {
        let mut candidate = snapshot.clone();
        match mutation {
            "tools" => candidate["tools"] = Value::Null,
            "binary" => candidate["tools"][0]["binary"] = Value::Null,
            "digest" => candidate["tools"][0]["executable_sha256"] = Value::Null,
            "relative-binary" => candidate["tools"][0]["binary"] = json!("relative-binary"),
            "different-executable" => {
                let other = project.join("build/other-binary");
                fs::write(&other, b"unrelated executable bytes").expect("different binary");
                candidate["tools"][0]["binary"] = json!(other);
                candidate["tools"][0]["executable_sha256"] =
                    json!(format!("{:x}", Sha256::digest(b"unrelated executable bytes")));
            }
            _ => {}
        }
        let bytes = if mutation == "malformed" {
            b"not JSON".to_vec()
        } else {
            serde_json::to_vec(&candidate).expect("snapshot JSON")
        };
        fs::write(&snapshot_path, &bytes).expect("snapshot");
        if mutation == "missing-pair" {
            context
                .as_object_mut()
                .expect("context object")
                .remove("runtime_snapshot_digest");
        } else {
            context["runtime_snapshot_digest"] = json!(format!("{:x}", Sha256::digest(&bytes)));
        }
        fs::write(&context_path, serde_json::to_vec(&context).expect("context JSON")).expect("context");
        assert_configuration_failure(
            project,
            &command(project)
                .arg("--run-context")
                .arg(&context_path)
                .arg("check")
                .output()
                .expect("context"),
            fragment,
        );
    }
}

#[cfg(unix)]
#[test]
fn test_deleted_working_directory_reports_errors_instead_of_panicking() {
    for relative_report in [false, true] {
        let directory = fixture();
        let project = directory.path();
        let dead = tempdir().expect("disposable working directory");
        let script = "import os,sys; os.chdir(sys.argv[1]); os.rmdir(sys.argv[1]); os.execv(sys.argv[2],sys.argv[2:])";
        let mut process = Command::new("/usr/bin/python3");
        process
            .args(["-c", script])
            .arg(dead.path())
            .arg(env!("CARGO_BIN_EXE_cpp-infra-style"));
        if relative_report {
            process
                .arg("--project")
                .arg(project)
                .args(["--report", "relative.json"]);
        } else {
            process
                .args(["--project", "."])
                .arg("--report")
                .arg(project.join("build/error.json"));
        }
        let output = process.arg("check").output().expect("deleted cwd CLI");
        assert_eq!(
            output.status.code(),
            Some(if relative_report { 3 } else { 2 }),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
        assert_eq!(fs::read_to_string(project.join("src/main.cpp")).expect("source"), GOOD);
    }
}

#[cfg(unix)]
#[test]
fn test_descriptor_exhaustion_is_an_infrastructure_failure_without_source_changes() {
    let directory = fixture();
    let project = directory.path();
    let script =
        "import os,resource,sys; resource.setrlimit(resource.RLIMIT_NOFILE,(9,9)); os.execv(sys.argv[1],sys.argv[1:])";
    let output = Command::new("/usr/bin/python3")
        .args(["-c", script])
        .arg(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--project")
        .arg(project)
        .arg("--report")
        .arg(project.join("build/result.json"))
        .arg("check")
        .output()
        .expect("resource-limited CLI");
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("Too many open files"));
    assert_eq!(report(project)["status"], "error");
    assert_eq!(fs::read_to_string(project.join("src/main.cpp")).expect("source"), GOOD);
}

#[cfg(unix)]
#[test]
fn test_disappearing_configuration_invalidates_loaded_policy() {
    let directory = fixture();
    let project = directory.path();
    let temporary = tempdir().expect("formatter directory");
    let policy = project.join(".infra/style/policy/rules.toml");
    let saved = fs::read(&policy).expect("policy bytes");
    let executable = wrapper(
        temporary.path(),
        "if '--version' in sys.argv: print('clang-format version 18.1.3')\nelse:\n    pathlib.Path('.infra/style/policy/rules.toml').rename('build/moved-policy.toml')\n    sys.stdout.write(sys.stdin.read())",
    );
    let output = command(project)
        .arg("fix")
        .env("CPP_INFRA_CLANG_FORMAT", executable)
        .output()
        .expect("fix");
    assert_configuration_failure(project, &output, "configuration changed");
    assert_eq!(
        fs::read(project.join("build/moved-policy.toml")).expect("user-moved policy"),
        saved
    );
    assert!(!policy.exists(), "checker must not recreate an externally moved input");
}

#[test]
fn test_overcomplex_glob_reports_a_configuration_error_instead_of_panicking() {
    let directory = fixture();
    let project = directory.path();
    let config = project.join(".infra/style/config.toml");
    let pattern = format!("src/{}", "[ab]".repeat(300_000));
    fs::write(
        &config,
        fs::read_to_string(&config)
            .expect("config")
            .replace("\"src/**\"", &format!("{pattern:?}")),
    )
    .expect("large user pattern");
    assert_configuration_failure(project, &check(project), "error building NFA");
    assert!(report(project)["commands"].as_array().expect("commands").is_empty());
}
