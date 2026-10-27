// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

use std::fs;
use std::path::Path;

use tempfile::tempdir;

use crate::config::load;
use crate::report::Report;
use crate::report::atomic_json_or_bytes;
use crate::style_error::StyleError;
use crate::tests::support::fixture;

#[test]
fn test_report_artifacts_require_successful_context_initialization() {
    let project = fixture();
    let mut report = Report::new(project.path(), None);
    assert!(matches!(report.create_log(0), Err(StyleError::Infrastructure(_))));
    assert!(matches!(
        report.artifact("diff", "style.patch", b"unvalidated output"),
        Err(StyleError::Infrastructure(_))
    ));
    assert!(
        !project.path().join("build").exists(),
        "unvalidated report lifecycle cannot write artifacts"
    );
}

#[test]
fn test_artifact_reuse_preserves_first_bytes_and_digest() {
    let project = fixture();
    let loaded = load(project.path()).expect("configuration");
    let mut report = Report::new(project.path(), None);
    report
        .initialize(project.path(), None, &loaded)
        .expect("initialized report");
    report
        .artifact("diff", "style.patch", b"first immutable patch")
        .expect("first artifact");
    let artifact = report.value["artifacts"][0].clone();
    assert!(matches!(
        report.artifact("diff", "style.patch", b"second patch"),
        Err(StyleError::Infrastructure(_))
    ));
    assert_eq!(
        fs::read(report.root.join(artifact["path"].as_str().expect("relative path"))).expect("original patch"),
        b"first immutable patch"
    );
    assert_eq!(report.value["artifacts"][0], artifact);
    assert_eq!(report.value["artifacts"].as_array().expect("artifacts").len(), 1);
}

#[test]
fn test_atomic_report_writer_rejects_directory_and_relative_destinations() {
    let root = tempdir().expect("output root");
    let directory = root.path().join("report.json");
    fs::create_dir(&directory).expect("directory target");
    fs::write(directory.join("business.txt"), b"preserved bytes").expect("business input");
    let error =
        atomic_json_or_bytes(&directory, b"must not replace a directory").expect_err("directory replacement forbidden");
    assert_eq!(error.exit_code(), 3);
    assert_eq!(
        fs::read(directory.join("business.txt")).expect("business bytes"),
        b"preserved bytes"
    );
    assert!(matches!(
        atomic_json_or_bytes(Path::new("relative.json"), b"{}"),
        Err(StyleError::Configuration(_))
    ));
    #[cfg(unix)]
    assert!(matches!(
        atomic_json_or_bytes(Path::new("/"), b"{}"),
        Err(StyleError::Configuration(_))
    ));
}
