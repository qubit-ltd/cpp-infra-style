// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

use std::process::Command;

use crate::run;
use crate::tests::support::fixture;

#[test]
fn test_library_cli_check() {
    if std::env::var_os("STYLE_REPEATED_RUN_CHILD").is_some() {
        assert_eq!(run(), 0, "first embedded CLI call should succeed");
        assert_eq!(
            run(),
            3,
            "a second signal-handler registration must return an infrastructure error"
        );
        return;
    }
    let project = fixture();
    let result = Command::new(std::env::current_exe().expect("test executable"))
        .arg("check")
        .current_dir(project.path())
        .env("STYLE_REPEATED_RUN_CHILD", "1")
        .output()
        .expect("isolated embedding process");
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
