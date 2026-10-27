// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Regressions for independently reproduced output and path safety defects.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;

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
    fs::create_dir_all(project.join("src")).expect("source directory");
    for name in ["rules.toml", "clang-format.yaml"] {
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("conf/policy").join(name),
            project.join(".infra/style/policy").join(name),
        )
        .expect("copy shipped policy");
    }
    fs::write(
        project.join(".infra/project.toml"),
        "schema=1\nkind=\"application\"\nsource_roots=[\"src\"]\n",
    )
    .expect("project config");
    fs::write(project.join(".infra/style/config.toml"),"schema=1\nextensions=[\"cpp\"]\ninclude=[\"src/**\"]\nrules=[\"format\",\"spdx-header\",\"final-newline\",\"trailing-whitespace\"]\nexceptions=\".infra/style/exceptions.toml\"\n").expect("style config");
    fs::write(
        project.join(".infra/style/exceptions.toml"),
        "schema=1\nexceptions=[]\n",
    )
    .expect("exceptions");
    fs::write(project.join("src/main.cpp"), GOOD).expect("source");
    directory
}

fn run(project: &Path, report: &str, context: Option<&Path>, command: &[&str]) -> Output {
    let mut process = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"));
    process
        .arg("--project")
        .arg(project)
        .arg("--report")
        .arg(project.join("build").join(report));
    if let Some(context) = context {
        process.arg("--run-context").arg(context);
    }
    process.args(command).output().expect("run style CLI")
}

fn context(project: &Path) -> (PathBuf, PathBuf) {
    let result = run(project, "baseline.json", None, &["check"]);
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value =
        serde_json::from_slice(&fs::read(project.join("build/baseline.json")).expect("baseline report"))
            .expect("report JSON");
    let root = project.join("build/infra-runs/shared");
    fs::create_dir_all(&root).expect("shared report root");
    let path = project.join("build/context.json");
    let context = json!({"schema":1,"run_id":"shared","task_id":"style","bundle_id":"development","project":project,"profile":null,"source_digest":report["source_digest"],"config_digest":report["config_digest"],"report_root":root});
    fs::write(&path, serde_json::to_vec(&context).expect("context JSON")).expect("context file");
    (root, path)
}

fn files(directory: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).expect("directory entries") {
            let entry = entry.expect("entry");
            let path = entry.path();
            if entry.file_type().expect("entry type").is_dir() {
                visit(root, &path, output);
            } else {
                output.insert(
                    path.strip_prefix(root).expect("relative path").to_path_buf(),
                    fs::read(path).expect("file bytes"),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(directory, directory, &mut result);
    result
}

#[cfg(unix)]
#[test]
fn test_s1_preexisting_log_symlinks_never_modify_source_or_external_files() {
    use std::os::unix::fs::symlink;
    let directory = fixture();
    let project = directory.path();
    let outside = tempdir().expect("outside");
    fs::write(outside.path().join("valuable.txt"), "external bytes must stay intact\n").expect("external file");
    let before_source = files(&project.join("src"));
    let before_outside = files(outside.path());
    let (root, path) = context(project);
    symlink(project.join("src/main.cpp"), root.join("command-0.log")).expect("source log symlink");
    symlink(outside.path().join("valuable.txt"), root.join("command-1.log")).expect("external log symlink");
    let result = run(project, "result.json", Some(&path), &["check"]);
    assert_eq!(
        files(&project.join("src")),
        before_source,
        "check must never alter linked source bytes"
    );
    assert_eq!(
        files(outside.path()),
        before_outside,
        "check must never alter linked external bytes"
    );
    assert_eq!(
        result.status.code(),
        Some(0),
        "isolated invocation should ignore hostile legacy log names: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[cfg(unix)]
#[test]
fn test_s2_unsafe_report_root_never_creates_external_files() {
    use std::os::unix::fs::symlink;
    let directory = fixture();
    let project = directory.path();
    let outside = tempdir().expect("outside");
    fs::write(outside.path().join("valuable.txt"), "external bytes must stay intact\n").expect("external file");
    let before_source = files(&project.join("src"));
    let before_outside = files(outside.path());
    symlink(outside.path(), project.join("build")).expect("build symlink");
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--project")
        .arg(project)
        .arg("check")
        .output()
        .expect("CLI");
    assert!(!output.status.success());
    assert_eq!(
        files(outside.path()),
        before_outside,
        "error reporting must not follow the refused build symlink"
    );
    assert_eq!(files(&project.join("src")), before_source);
}

#[cfg(unix)]
#[test]
fn test_s3_reused_context_preserves_every_previous_artifact_digest() {
    use std::os::unix::fs::PermissionsExt;
    let directory = fixture();
    let project = directory.path();
    let wrapper = tempdir().expect("wrapper directory");
    let executable = wrapper.path().join("formatter.py");
    fs::write(&executable,"#!/usr/bin/python3\nimport os,subprocess,sys\nmarker=os.environ['REVIEW_MARKER']\nprint(marker,file=sys.stderr)\nresult=subprocess.run(['/usr/bin/clang-format']+sys.argv[1:],input=sys.stdin.buffer.read(),capture_output=True)\nsys.stderr.buffer.write(result.stderr)\nsys.stdout.buffer.write(result.stdout.replace(b'return 0',('return '+marker).encode()) if '--version' not in sys.argv else result.stdout)\nsys.exit(result.returncode)\n").expect("formatter wrapper");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("wrapper executable");
    let (root, path) = context(project);
    let before = files(&project.join("src"));
    for marker in ["1", "2"] {
        let result = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
            .arg("--project")
            .arg(project)
            .arg("--report")
            .arg(project.join(format!("build/run{marker}.json")))
            .arg("--run-context")
            .arg(&path)
            .args(["fix", "--dry-run"])
            .env("CPP_INFRA_CLANG_FORMAT", &executable)
            .env("REVIEW_MARKER", marker)
            .output()
            .expect("CLI");
        assert_eq!(
            result.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    for marker in ["1", "2"] {
        let report: Value =
            serde_json::from_slice(&fs::read(project.join(format!("build/run{marker}.json"))).expect("report"))
                .expect("report JSON");
        assert_eq!(
            report["report_root"],
            json!(root),
            "public report_root remains the shared run root"
        );
        for artifact in report["artifacts"].as_array().expect("artifact list") {
            let path = artifact["path"].as_str().expect("relative artifact path");
            let digest = format!(
                "{:x}",
                Sha256::digest(fs::read(root.join(path)).expect("artifact bytes"))
            );
            assert_eq!(
                artifact["sha256"], digest,
                "run {marker} artifact {path} changed after retry"
            );
            assert!(
                path.starts_with("style/"),
                "each invocation owns a private style artifact directory"
            );
        }
    }
    assert_eq!(files(&project.join("src")), before, "dry-run cannot edit source");
}

#[cfg(unix)]
#[test]
fn test_s4_literal_backslash_paths_are_rejected_without_aliasing_exceptions() {
    let directory = fixture();
    let project = directory.path();
    fs::create_dir(project.join("src/a")).expect("nested source");
    fs::write(project.join("src/a/b.cpp"), "int f() { return 0; }\n").expect("ordinary source");
    fs::write(project.join("src/a\\b.cpp"), "int g() { return 0; }\n").expect("literal backslash source");
    fs::write(
        project.join(".infra/style/exceptions.toml"),
        "schema=1\n[[exceptions]]\nrule=\"spdx-header\"\npath=\"src/a/b.cpp\"\nreason=\"This exact file only\"\n",
    )
    .expect("exact exception");
    let before = files(&project.join("src"));
    let result = run(project, "result.json", None, &["check"]);
    assert_eq!(
        result.status.code(),
        Some(2),
        "literal backslashes must not become protocol path separators"
    );
    assert!(String::from_utf8_lossy(&result.stderr).contains("backslash"));
    assert_eq!(files(&project.join("src")), before);
}

#[cfg(unix)]
#[test]
fn test_s4_digests_reject_literal_backslashes_outside_selected_sources() {
    for relative in ["notes\\release.txt", ".infra/style/policy/extra\\policy.txt"] {
        let directory = fixture();
        let project = directory.path();
        fs::write(project.join(relative), "literal path bytes\n").expect("literal backslash input");
        let before_source = files(&project.join("src"));
        let before_config = files(&project.join(".infra"));
        let result = run(project, "result.json", None, &["check"]);
        assert_eq!(
            result.status.code(),
            Some(2),
            "digest accepted unrepresentable protocol path {relative}"
        );
        assert!(String::from_utf8_lossy(&result.stderr).contains("backslash"));
        assert_eq!(files(&project.join("src")), before_source);
        assert_eq!(files(&project.join(".infra")), before_config);
        assert_eq!(
            fs::read(project.join(relative)).expect("input bytes"),
            b"literal path bytes\n"
        );
    }
}

#[test]
fn test_r1_nested_reserved_directory_names_are_checked_and_invalidate_context() {
    for directory_name in ["build", "target", ".git", ".infra"] {
        let directory = fixture();
        let project = directory.path();
        let relative = format!("src/{directory_name}/hidden.cpp");
        let hidden = project.join(&relative);
        fs::create_dir_all(hidden.parent().expect("source parent")).expect("nested source directory");
        fs::write(&hidden, "int  hidden(){return 1;}  ").expect("bad nested source");
        let checked = run(project, "bad.json", None, &["check"]);
        assert_eq!(
            checked.status.code(),
            Some(1),
            "configured nested source omitted: {relative}"
        );
        let report: Value =
            serde_json::from_slice(&fs::read(project.join("build/bad.json")).expect("report")).expect("JSON report");
        assert!(
            report["data"]["files"]
                .as_array()
                .expect("selected files")
                .contains(&json!(relative))
        );
        fs::write(&hidden, GOOD).expect("valid nested source");
        let (_, saved_context) = context(project);
        fs::write(&hidden, GOOD.replace("return 0", "return 1")).expect("nested source edit");
        assert_eq!(
            run(project, "stale.json", Some(&saved_context), &["check"])
                .status
                .code(),
            Some(2),
            "nested source must be part of source_digest: {relative}"
        );
        assert_eq!(run(project, "fresh.json", None, &["check"]).status.code(), Some(0));
    }
}

#[test]
fn test_r2_only_exact_root_transaction_directories_are_ignored() {
    for relative in [
        ".infra/.tmp/staged",
        ".infra/transactions/staged",
        ".infra/.transactions/staged",
    ] {
        let directory = fixture();
        let project = directory.path();
        let temporary = project.join(relative);
        fs::create_dir_all(temporary.parent().expect("transaction parent")).expect("transaction directory");
        fs::write(&temporary, "temporary-1").expect("temporary input");
        let (_, saved_context) = context(project);
        fs::write(&temporary, "temporary-2").expect("temporary edit");
        let output = run(project, "result.json", Some(&saved_context), &["check"]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "controlled transaction changed config identity: {relative}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn test_r2_nested_and_similarly_named_transaction_inputs_are_tracked() {
    for relative in [
        ".infra/style/policy/transactions/rule.txt",
        ".infra/style/policy/.transactions/rule.txt",
        ".infra/style/policy/.transaction-extra/rule.txt",
        ".infra/.transaction-journal/rule.txt",
    ] {
        let directory = fixture();
        let project = directory.path();
        let input = project.join(relative);
        fs::create_dir_all(input.parent().expect("policy parent")).expect("policy directory");
        fs::write(&input, "real-input-1").expect("configuration input");
        let (_, saved_context) = context(project);
        fs::write(&input, "real-input-2").expect("configuration edit");
        let output = run(project, "result.json", Some(&saved_context), &["check"]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "real configuration was incorrectly excluded: {relative}"
        );
    }
}

#[test]
fn test_source_exclusions_root_reports_share_the_same_fresh_source_identity() {
    let directory = fixture();
    let project = directory.path();
    let run_root_report = |name: &str, context_path: Option<&Path>| {
        let mut process = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"));
        process
            .arg("--project")
            .arg(project)
            .arg("--report")
            .arg(project.join(name));
        if let Some(path) = context_path {
            process.arg("--run-context").arg(path);
        }
        process.arg("check").output().expect("CLI")
    };
    assert_eq!(run_root_report("result.json", None).status.code(), Some(0));
    let first: Value =
        serde_json::from_slice(&fs::read(project.join("result.json")).expect("root report")).expect("JSON");
    assert_eq!(first["source_excluded_paths"], json!(["result.json"]));
    let saved_context = project.join("build/context.json");
    let mut value = json!({"schema":1,"run_id":"excluded-run","task_id":"style","bundle_id":"development","project":project,"profile":null,"source_digest":first["source_digest"],"config_digest":first["config_digest"],"report_root":project.join("build/infra-runs/excluded-run"),"source_excluded_paths":first["source_excluded_paths"]});
    fs::write(&saved_context, serde_json::to_vec(&value).expect("context JSON")).expect("context");
    let second = run_root_report("second.json", Some(&saved_context));
    assert_eq!(
        second.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second: Value =
        serde_json::from_slice(&fs::read(project.join("second.json")).expect("second root report")).expect("JSON");
    assert_eq!(second["source_excluded_paths"], json!(["result.json", "second.json"]));
    assert_eq!(second["source_digest"], first["source_digest"]);
    assert_eq!(second["config_digest"], first["config_digest"]);
    value["source_excluded_paths"] = second["source_excluded_paths"].clone();
    fs::write(&saved_context, serde_json::to_vec(&value).expect("context JSON")).expect("context");
    assert_eq!(
        run(project, "fresh.json", Some(&saved_context), &["check"])
            .status
            .code(),
        Some(0)
    );
    fs::write(project.join("business.json"), "{\"ordinary_input\":true}\n").expect("business input");
    assert_eq!(
        run(project, "stale.json", Some(&saved_context), &["check"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn test_source_exclusions_are_sorted_deduplicated_and_accept_family_reports() {
    let directory = fixture();
    let project = directory.path();
    let (_, saved_context) = context(project);
    let mut context: Value = serde_json::from_slice(&fs::read(&saved_context).expect("context")).expect("JSON");
    context["source_excluded_paths"] = json!(["z-report.json", "a-report.json", "z-report.json"]);
    fs::write(&saved_context, serde_json::to_vec(&context).expect("JSON")).expect("context");
    assert_eq!(
        run(project, "result.json", Some(&saved_context), &["check"])
            .status
            .code(),
        Some(0)
    );
    let mut report: Value =
        serde_json::from_slice(&fs::read(project.join("build/result.json")).expect("report")).expect("JSON");
    assert_eq!(
        report["source_excluded_paths"],
        json!(["a-report.json", "z-report.json"])
    );
    report["tool"]["name"] = json!("cpp-infra-coverage");
    fs::write(
        project.join("family-report.json"),
        serde_json::to_vec(&report).expect("family report"),
    )
    .expect("family report file");
    let result = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--project")
        .arg(project)
        .arg("--report")
        .arg(project.join("family-report.json"))
        .arg("check")
        .output()
        .expect("CLI");
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn test_source_exclusions_cannot_hide_inputs_aliases_directories_or_business_json() {
    for excluded in [
        "src/input.json",
        ".infra/input.json",
        "conan/input.json",
        "CMakePresets.json",
        "README.json",
        "../outside.json",
        "alias\\report.json",
        "alias//report.json",
        "",
        "directory.json",
        "business.json",
    ] {
        let directory = fixture();
        let project = directory.path();
        fs::create_dir(project.join("directory.json")).expect("JSON-looking directory");
        fs::write(project.join("business.json"), "{\"business\":true}\n").expect("ordinary business JSON");
        let (_, saved_context) = context(project);
        let mut context: Value = serde_json::from_slice(&fs::read(&saved_context).expect("context")).expect("JSON");
        context["source_excluded_paths"] = json!([excluded]);
        fs::write(&saved_context, serde_json::to_vec(&context).expect("JSON")).expect("context");
        let before_source = files(&project.join("src"));
        let before_config = files(&project.join(".infra"));
        let output = run(project, "result.json", Some(&saved_context), &["check"]);
        assert_eq!(output.status.code(), Some(2), "invalid exclusion accepted: {excluded}");
        assert_eq!(files(&project.join("src")), before_source);
        assert_eq!(files(&project.join(".infra")), before_config);
        assert_eq!(
            fs::read(project.join("business.json")).expect("business bytes"),
            b"{\"business\":true}\n"
        );
    }
}

#[test]
fn test_report_output_cannot_create_reserved_configuration_or_readme_inputs() {
    for report in [
        "CMakePresets.json",
        "CMakeUserPresets.json",
        "README.json",
        "conan/input.json",
        "cmake/input.json",
    ] {
        let directory = fixture();
        let project = directory.path();
        let before_source = files(&project.join("src"));
        let before_config = files(&project.join(".infra"));
        let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
            .arg("--project")
            .arg(project)
            .arg("--report")
            .arg(project.join(report))
            .arg("check")
            .output()
            .expect("CLI");
        assert_eq!(
            output.status.code(),
            Some(2),
            "reserved input accepted as a report output: {report}"
        );
        assert!(!project.join(report).exists(), "refused output must not be written");
        assert_eq!(files(&project.join("src")), before_source);
        assert_eq!(files(&project.join(".infra")), before_config);
    }
}

#[test]
fn test_source_exclusions_protect_all_declared_ownership_roots() {
    for relative in ["tests/report.json", "generated/report.json", "vendor/report.json"] {
        let directory = fixture();
        let project = directory.path();
        for root in ["tests", "generated", "vendor"] {
            fs::create_dir(project.join(root)).expect("ownership root");
        }
        fs::write(project.join(".infra/project.toml"),"schema=1\nkind=\"application\"\nsource_roots=[\"src\"]\ntest_roots=[\"tests\"]\ngenerated_roots=[\"generated\"]\nthird_party_roots=[\"vendor\"]\n").expect("ownership config");
        let (_, saved_context) = context(project);
        let mut context: Value = serde_json::from_slice(&fs::read(&saved_context).expect("context")).expect("JSON");
        context["source_excluded_paths"] = json!([relative]);
        fs::write(&saved_context, serde_json::to_vec(&context).expect("JSON")).expect("context");
        let result = run(project, "result.json", Some(&saved_context), &["check"]);
        assert_eq!(result.status.code(), Some(2), "ownership root excluded: {relative}");
        assert!(!project.join(relative).exists());
        assert_eq!(fs::read(project.join("src/main.cpp")).expect("source"), GOOD.as_bytes());
    }
}

#[cfg(unix)]
#[test]
fn test_source_exclusions_and_external_outputs_reject_symlink_input_aliases() {
    use std::os::unix::fs::symlink;
    let directory = fixture();
    let project = directory.path();
    let outside = tempdir().expect("outside");
    symlink(project.join("src"), project.join("alias")).expect("source alias");
    let (_, saved_context) = context(project);
    let mut context: Value = serde_json::from_slice(&fs::read(&saved_context).expect("context")).expect("JSON");
    context["source_excluded_paths"] = json!(["alias/report.json"]);
    fs::write(&saved_context, serde_json::to_vec(&context).expect("JSON")).expect("context");
    let before = files(&project.join("src"));
    assert_eq!(
        run(project, "result.json", Some(&saved_context), &["check"])
            .status
            .code(),
        Some(2)
    );
    symlink(project.join("src"), outside.path().join("reports")).expect("external source alias");
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--project")
        .arg(project)
        .arg("--report")
        .arg(outside.path().join("reports/output.json"))
        .arg("check")
        .output()
        .expect("CLI");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(files(&project.join("src")), before);
}

#[cfg(unix)]
#[test]
fn test_source_exclusions_revalidate_file_roles_before_fresh_identity() {
    use std::os::unix::fs::PermissionsExt;
    let directory = fixture();
    let project = directory.path();
    let wrapper = tempdir().expect("formatter directory");
    let (_, saved_context) = context(project);
    let mut context: Value = serde_json::from_slice(&fs::read(&saved_context).expect("context")).expect("JSON");
    fs::copy(project.join("build/baseline.json"), project.join("old-report.json")).expect("existing public report");
    context["source_excluded_paths"] = json!(["old-report.json"]);
    fs::write(&saved_context, serde_json::to_vec(&context).expect("JSON")).expect("context");
    let executable = wrapper.path().join("formatter.py");
    fs::write(&executable,"#!/usr/bin/python3\nimport pathlib,sys\nif '--version' in sys.argv: print('clang-format version 18.1.3')\nelse:\n    pathlib.Path('old-report.json').write_text('new business bytes\\n')\n    sys.stdout.write(sys.stdin.read())\n").expect("formatter");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("executable formatter");
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--project")
        .arg(project)
        .arg("--report")
        .arg(project.join("build/result.json"))
        .arg("--run-context")
        .arg(&saved_context)
        .arg("check")
        .env("CPP_INFRA_CLANG_FORMAT", &executable)
        .output()
        .expect("CLI");
    assert_eq!(
        output.status.code(),
        Some(2),
        "excluded files must still be non-input public reports at fresh validation"
    );
    assert_eq!(
        fs::read(project.join("old-report.json")).expect("business input"),
        b"new business bytes\n"
    );
    assert_eq!(fs::read(project.join("src/main.cpp")).expect("source"), GOOD.as_bytes());
}

#[cfg(unix)]
#[test]
fn test_report_writer_preserves_business_file_created_during_check() {
    use std::os::unix::fs::PermissionsExt;
    let directory = fixture();
    let project = directory.path();
    let wrapper = tempdir().expect("formatter directory");
    let executable = wrapper.path().join("formatter.py");
    fs::write(&executable,"#!/usr/bin/python3\nimport pathlib,sys\nif '--version' in sys.argv: print('clang-format version 18.1.3')\nelse:\n    pathlib.Path('result.json').write_text('new business bytes\\n')\n    sys.stdout.write(sys.stdin.read())\n").expect("formatter");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("executable formatter");
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
        .arg("--project")
        .arg(project)
        .arg("--report")
        .arg(project.join("result.json"))
        .arg("check")
        .env("CPP_INFRA_CLANG_FORMAT", &executable)
        .output()
        .expect("CLI");
    assert_eq!(
        fs::read(project.join("result.json")).expect("concurrent business file"),
        b"new business bytes\n",
        "final report must not overwrite a newly created business input"
    );
    assert_eq!(
        output.status.code(),
        Some(3),
        "unsafe report persistence is an IO failure with stderr diagnostics"
    );
    assert_eq!(fs::read(project.join("src/main.cpp")).expect("source"), GOOD.as_bytes());
}

#[test]
fn test_f1_active_exception_inputs_cannot_live_in_untracked_directories() {
    for relative in [
        "build/exceptions.toml",
        "target/exceptions.toml",
        ".git/exceptions.toml",
        ".infra/.tmp/exceptions.toml",
        ".infra/transactions/exceptions.toml",
        ".infra/.transactions/exceptions.toml",
    ] {
        let directory = fixture();
        let project = directory.path();
        let input = project.join(relative);
        fs::create_dir_all(input.parent().expect("configuration parent")).expect("configuration directory");
        fs::write(&input, "schema=1\nexceptions=[]\n").expect("exception configuration");
        let config = project.join(".infra/style/config.toml");
        fs::write(
            &config,
            fs::read_to_string(&config)
                .expect("config")
                .replace(".infra/style/exceptions.toml", relative),
        )
        .expect("custom exception path");
        let before_source = files(&project.join("src"));
        let before_input = fs::read(&input).expect("configuration bytes");
        let output = run(project, "result.json", None, &["check"]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "active input in an untracked area was accepted: {relative}"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("configuration input"));
        assert_eq!(files(&project.join("src")), before_source);
        assert_eq!(fs::read(input).expect("configuration after check"), before_input);
    }
}

#[cfg(unix)]
#[test]
fn test_f1_all_active_configuration_files_reject_symlink_inputs() {
    use std::os::unix::fs::symlink;
    for relative in [
        ".infra/project.toml",
        ".infra/style/config.toml",
        ".infra/style/policy/rules.toml",
        ".infra/style/policy/clang-format.yaml",
        ".infra/style/exceptions.toml",
    ] {
        let directory = fixture();
        let project = directory.path();
        fs::create_dir(project.join("build")).expect("build directory");
        let original = project.join(relative);
        let cached = project.join("build/cached-input");
        fs::rename(&original, &cached).expect("move temporary fixture input");
        symlink(&cached, &original).expect("cached input symlink");
        let before = fs::read(&cached).expect("cached bytes");
        let result = run(project, "result.json", None, &["check"]);
        assert_eq!(
            result.status.code(),
            Some(2),
            "symlinked active configuration was accepted: {relative}"
        );
        assert_eq!(fs::read(cached).expect("cached bytes after check"), before);
        assert_eq!(fs::read(project.join("src/main.cpp")).expect("source"), GOOD.as_bytes());
    }
}

#[cfg(unix)]
#[test]
fn test_f1_active_configuration_parent_symlinks_are_rejected() {
    use std::os::unix::fs::symlink;
    for relative in [".infra/style/policy", ".infra/style", ".infra"] {
        let directory = fixture();
        let project = directory.path();
        fs::create_dir(project.join("build")).expect("build directory");
        let original = project.join(relative);
        let cached = project.join("build/cached-directory");
        fs::rename(&original, &cached).expect("move temporary fixture directory");
        symlink(&cached, &original).expect("configuration parent symlink");
        let before = files(&cached);
        let result = run(project, "result.json", None, &["check"]);
        assert_eq!(
            result.status.code(),
            Some(2),
            "symlinked configuration parent was accepted: {relative}"
        );
        assert_eq!(files(&cached), before);
        assert_eq!(fs::read(project.join("src/main.cpp")).expect("source"), GOOD.as_bytes());
    }
}

#[test]
fn test_f1_regular_nested_build_configuration_remains_tracked() {
    let directory = fixture();
    let project = directory.path();
    fs::create_dir_all(project.join("src/build")).expect("nested source directory");
    let input = project.join("src/build/exceptions.toml");
    fs::write(&input, "schema=1\nexceptions=[]\n").expect("tracked exception input");
    let config = project.join(".infra/style/config.toml");
    fs::write(
        &config,
        fs::read_to_string(&config)
            .expect("config")
            .replace(".infra/style/exceptions.toml", "src/build/exceptions.toml"),
    )
    .expect("tracked custom input");
    let (_, saved_context) = context(project);
    fs::write(&input, "schema=1\nexceptions=[]\n# a tracked input changed\n").expect("input edit");
    assert_eq!(
        run(project, "result.json", Some(&saved_context), &["check"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(run(project, "fresh.json", None, &["check"]).status.code(), Some(0));
}

#[test]
fn test_source_exclusion_raw_aliases_are_rejected_before_files_appear() {
    for raw in [
        "business.json/",
        "business.json//",
        "./business.json",
        "nested/./business.json",
        "nested//business.json",
        "../business.json",
        "/business.json",
        "C:/business.json",
        "C:business.json",
        "C:\\business.json",
        "//server/business.json",
    ] {
        let directory = fixture();
        let project = directory.path();
        let (_, saved_context) = context(project);
        let mut context: Value = serde_json::from_slice(&fs::read(&saved_context).expect("context")).expect("JSON");
        context["source_excluded_paths"] = json!([raw]);
        fs::write(&saved_context, serde_json::to_vec(&context).expect("JSON")).expect("context");
        let before = run(project, "before.json", Some(&saved_context), &["check"]);
        assert_eq!(
            before.status.code(),
            Some(2),
            "noncanonical raw exclusion accepted before the file exists: {raw}"
        );
        assert!(
            String::from_utf8_lossy(&before.stderr).contains("source_excluded_paths"),
            "raw field must be rejected before exists or freshness checks: {raw}"
        );
        fs::write(project.join("business.json"), "{\"business_input\":true}\n").expect("new business file");
        let after = run(project, "after.json", Some(&saved_context), &["check"]);
        assert_eq!(
            after.status.code(),
            Some(2),
            "raw alias hid a subsequent business file: {raw}"
        );
        assert!(String::from_utf8_lossy(&after.stderr).contains("source_excluded_paths"));
        assert_eq!(
            fs::read(project.join("business.json")).expect("business bytes"),
            b"{\"business_input\":true}\n"
        );
        assert_eq!(
            fs::read(project.join("src/main.cpp")).expect("source bytes"),
            GOOD.as_bytes()
        );
    }
}

#[cfg(unix)]
#[test]
fn test_a1_fix_rejects_revoked_custom_exceptions_before_or_after_source_changes() {
    use std::os::unix::fs::PermissionsExt;
    for unformatted in [false, true] {
        for mutate_on in [1, 2] {
            let directory = fixture();
            let project = directory.path();
            let wrapper = tempdir().expect("wrapper directory");
            fs::create_dir_all(project.join("src/build")).expect("tracked custom config directory");
            let exceptions = project.join("src/build/exceptions.toml");
            fs::write(&exceptions,"schema=1\n[[exceptions]]\nrule=\"spdx-header\"\npath=\"src/main.cpp\"\nreason=\"Approved original license\"\n").expect("approved exception");
            let config = project.join(".infra/style/config.toml");
            fs::write(
                &config,
                fs::read_to_string(&config)
                    .expect("config")
                    .replace(".infra/style/exceptions.toml", "src/build/exceptions.toml"),
            )
            .expect("custom exception location");
            let initial = if unformatted {
                "int  main(){return 0;}\n"
            } else {
                "int main() { return 0; }\n"
            };
            fs::write(project.join("src/main.cpp"), initial).expect("source");
            assert_eq!(
                run(project, "baseline.json", None, &["check"]).status.code(),
                Some(if unformatted { 1 } else { 0 })
            );
            let baseline: Value =
                serde_json::from_slice(&fs::read(project.join("build/baseline.json")).expect("baseline"))
                    .expect("JSON");
            let saved_context = project.join("build/context.json");
            fs::write(&saved_context,serde_json::to_vec(&json!({"schema":1,"run_id":"a1-run","task_id":"style","bundle_id":"development","project":project,"profile":null,"source_digest":baseline["source_digest"],"config_digest":baseline["config_digest"],"report_root":project.join("build/infra-runs/a1-run")})).expect("context JSON")).expect("context");
            let executable = wrapper.path().join("formatter.py");
            fs::write(&executable,format!("#!/usr/bin/python3\nfrom pathlib import Path\nimport subprocess,sys\nif '--version' not in sys.argv:\n    count=Path({:?})\n    value=int(count.read_text())+1 if count.exists() else 1\n    count.write_text(str(value))\n    if value=={mutate_on}: Path('src/build/exceptions.toml').write_text('schema=1\\nexceptions=[]\\n')\nresult=subprocess.run(['/usr/bin/clang-format']+sys.argv[1:],input=sys.stdin.buffer.read(),capture_output=True)\nsys.stdout.buffer.write(result.stdout)\nsys.stderr.buffer.write(result.stderr)\nsys.exit(result.returncode)\n",wrapper.path().join("calls").to_str().expect("counter path"))).expect("real formatter wrapper");
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("executable");
            let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
                .arg("--project")
                .arg(project)
                .arg("--report")
                .arg(project.join("build/result.json"))
                .arg("--run-context")
                .arg(&saved_context)
                .arg("fix")
                .env("CPP_INFRA_CLANG_FORMAT", &executable)
                .output()
                .expect("fix");
            assert_eq!(
                output.status.code(),
                Some(2),
                "revoked exception certified by fix: unformatted={unformatted}, phase={mutate_on}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stderr).contains("configuration changed"));
            assert_eq!(
                fs::read(&exceptions).expect("revoked exception bytes"),
                b"schema=1\nexceptions=[]\n"
            );
            let expected = if unformatted && mutate_on == 2 {
                "int main() { return 0; }\n"
            } else {
                initial
            };
            assert_eq!(
                fs::read_to_string(project.join("src/main.cpp")).expect("source"),
                expected,
                "policy failure cannot overwrite unrelated source edits"
            );
            let report: Value =
                serde_json::from_slice(&fs::read(project.join("build/result.json")).expect("report")).expect("JSON");
            assert_eq!(report["status"], "error");
            assert_eq!(
                report["data"]["changed_files"],
                if unformatted && mutate_on == 2 {
                    json!(["src/main.cpp"])
                } else {
                    json!([])
                }
            );
            assert_eq!(run(project, "fresh.json", None, &["check"]).status.code(), Some(1));
        }
    }
}

#[cfg(unix)]
#[test]
fn test_a1_configuration_identity_changes_are_not_absorbed_by_fix() {
    use std::os::unix::fs::PermissionsExt;
    for relative in [
        ".infra/project.toml",
        ".infra/style/config.toml",
        ".infra/style/policy/rules.toml",
        ".infra/style/policy/clang-format.yaml",
        ".infra/style/exceptions.toml",
    ] {
        let directory = fixture();
        let project = directory.path();
        let wrapper = tempdir().expect("wrapper directory");
        let executable = wrapper.path().join("formatter.py");
        let original = fs::read(project.join(relative)).expect("configuration bytes");
        fs::write(&executable,format!("#!/usr/bin/python3\nfrom pathlib import Path\nimport subprocess,sys\nif '--version' not in sys.argv:\n    path=Path({relative:?})\n    replacement=path.with_name(path.name+'.replacement')\n    replacement.write_bytes(path.read_bytes())\n    replacement.replace(path)\nresult=subprocess.run(['/usr/bin/clang-format']+sys.argv[1:],input=sys.stdin.buffer.read(),capture_output=True)\nsys.stdout.buffer.write(result.stdout)\nsys.stderr.buffer.write(result.stderr)\nsys.exit(result.returncode)\n")).expect("identity replacement wrapper");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("executable");
        let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-style"))
            .arg("--project")
            .arg(project)
            .arg("--report")
            .arg(project.join("build/result.json"))
            .arg("fix")
            .env("CPP_INFRA_CLANG_FORMAT", executable)
            .output()
            .expect("fix");
        assert_eq!(
            output.status.code(),
            Some(2),
            "replacement identity ignored for {relative}"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("configuration changed"));
        assert_eq!(fs::read(project.join(relative)).expect("replacement bytes"), original);
        assert_eq!(fs::read(project.join("src/main.cpp")).expect("source"), GOOD.as_bytes());
    }
}

#[test]
fn test_a1_fix_never_formats_a_file_consumed_as_configuration() {
    let directory = fixture();
    let project = directory.path();
    let input = project.join("src/exceptions.cpp");
    let original = "schema=1\nexceptions=[]\n";
    fs::write(&input, original).expect("custom configuration with selected suffix");
    let config = project.join(".infra/style/config.toml");
    fs::write(
        &config,
        fs::read_to_string(&config)
            .expect("config")
            .replace(".infra/style/exceptions.toml", "src/exceptions.cpp"),
    )
    .expect("custom exception configuration");
    let result = run(project, "result.json", None, &["fix"]);
    assert_eq!(
        fs::read_to_string(&input).expect("configuration after fix"),
        original,
        "fix must never rewrite an active configuration input even if its suffix is selected"
    );
    assert_eq!(result.status.code(), Some(2));
    assert_eq!(fs::read(project.join("src/main.cpp")).expect("source"), GOOD.as_bytes());
}
