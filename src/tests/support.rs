// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

use std::fs;
use std::path::Path;

use tempfile::TempDir;
use tempfile::tempdir;

pub(super) fn fixture() -> TempDir {
    let project = tempdir().expect("internal contract project");
    fs::create_dir_all(project.path().join(".infra/style/policy")).expect("policy directory");
    fs::create_dir(project.path().join("src")).expect("source directory");
    for file in ["rules.toml", "clang-format.yaml"] {
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("conf/policy").join(file),
            project.path().join(".infra/style/policy").join(file),
        )
        .expect("shipped policy");
    }
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("conf/config.toml"),
        project.path().join(".infra/style/config.toml"),
    )
    .expect("shipped config");
    fs::write(
        project.path().join(".infra/project.toml"),
        "schema=1\nkind=\"application\"\nsource_roots=[\"src\"]\n",
    )
    .expect("project configuration");
    fs::write(
        project.path().join("src/main.cpp"),
        "// SPDX-License-Identifier: Apache-2.0\n// Copyright (c) 2026 Qubit\nint main() { return 0; }\n",
    )
    .expect("source");
    project
}
