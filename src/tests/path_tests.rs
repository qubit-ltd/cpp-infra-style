// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use tempfile::tempdir;

use crate::safe_write::replace;
use crate::scope::Source;
use crate::scope::protocol_path;
use crate::style_error::StyleError;

#[test]
fn test_protocol_paths_reject_outside_empty_and_parent_components() {
    let root = tempdir().expect("root");
    let outside = tempdir().expect("outside");
    for invalid in [
        outside.path().join("input.cpp"),
        root.path().to_path_buf(),
        root.path().join("../input.cpp"),
    ] {
        assert!(
            matches!(protocol_path(root.path(), &invalid), Err(StyleError::Configuration(_))),
            "invalid protocol identity: {}",
            invalid.display()
        );
    }
    assert_eq!(
        protocol_path(root.path(), &root.path().join("src/build/source.cpp")).expect("nested identity"),
        "src/build/source.cpp"
    );
}

#[cfg(unix)]
#[test]
fn test_source_writer_refuses_a_root_path_without_creating_output() {
    let root = tempdir().expect("metadata fixture");
    let source = Source {
        path: PathBuf::from("/"),
        relative: "invalid-root".into(),
        original: Vec::new(),
        permissions: fs::metadata(root.path()).expect("metadata").permissions(),
    };
    let error = replace(&source, b"must never be persisted").expect_err("root cannot be replaced");
    assert!(matches!(error, StyleError::Configuration(_)));
    assert!(Path::new("/").is_dir());
    assert_eq!(fs::read_dir(root.path()).expect("fixture").count(), 0);
}
