// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Deterministic source enumeration and common project input digests.

use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use sha2::Digest;
use sha2::Sha256;

use crate::config::Loaded;
use crate::style_error::StyleError;

pub(crate) struct Source {
    pub path: PathBuf,
    pub relative: String,
    pub original: Vec<u8>,
    pub permissions: fs::Permissions,
}

pub(crate) fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Encode actual path components, rejecting literal characters that would alias
/// protocol separators rather than rewriting distinct filesystem names.
pub(crate) fn protocol_path(root: &Path, path: &Path) -> Result<String, StyleError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|error| StyleError::Configuration(error.to_string()))?;
    let mut components = Vec::new();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(StyleError::Configuration(
                "protocol path must be normalized and relative".into(),
            ));
        };
        let component = component
            .to_str()
            .ok_or_else(|| StyleError::Configuration("non-UTF8 project path".into()))?;
        if component.contains('\\') {
            return Err(StyleError::Configuration(format!(
                "literal backslash is unsupported in protocol paths: {}",
                path.display()
            )));
        }
        components.push(component);
    }
    if components.is_empty() {
        return Err(StyleError::Configuration("empty protocol path".into()));
    }
    Ok(components.join("/"))
}

fn entries(path: &Path) -> Result<Vec<fs::DirEntry>, StyleError> {
    let mut values: Vec<fs::DirEntry> = fs::read_dir(path)?.collect::<Result<_, _>>()?;
    values.sort_by_key(fs::DirEntry::file_name);
    Ok(values)
}

pub(crate) fn scan(root: &Path, loaded: &Loaded) -> Result<Vec<Source>, StyleError> {
    let mut paths = Vec::new();
    for relative in loaded.project.source_roots.iter().chain(&loaded.project.test_roots) {
        walk_sources(root, &root.join(relative), loaded, &mut paths)?;
    }
    paths.sort_by(|a, b| a.relative.as_bytes().cmp(b.relative.as_bytes()));
    paths.dedup_by(|a, b| a.path == b.path);
    if paths.is_empty() {
        return Err(StyleError::Configuration(
            "configured source scope selected no files".into(),
        ));
    }
    Ok(paths)
}

fn walk_sources(root: &Path, directory: &Path, loaded: &Loaded, files: &mut Vec<Source>) -> Result<(), StyleError> {
    let directory_relative = protocol_path(root, directory)?;
    if directory_relative
        .split('/')
        .next()
        .is_some_and(|component| [".git", ".infra", "build", "target"].contains(&component))
    {
        return Ok(());
    }
    for entry in entries(directory)? {
        let path = entry.path();
        let file_type = entry.file_type()?;
        let relative = protocol_path(root, &path)?;
        if file_type.is_symlink() {
            if !fs::canonicalize(&path)?.starts_with(root) {
                return Err(StyleError::Configuration(format!(
                    "symlink escapes project: {relative}"
                )));
            }
            continue;
        }
        if file_type.is_dir() {
            walk_sources(root, &path, loaded, files)?;
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| loaded.config.extensions.iter().any(|value| value == e))
            && loaded.include.is_match(&relative)
            && !loaded.exclude.is_match(&relative)
        {
            let original = fs::read(&path)
                .map_err(|e| StyleError::Infrastructure(format!("cannot read source {relative}: {e}")))?;
            std::str::from_utf8(&original)
                .map_err(|e| StyleError::Configuration(format!("non-UTF8 source {relative}: {e}")))?;
            files.push(Source {
                path,
                relative,
                original,
                permissions: entry.metadata()?.permissions(),
            });
        }
    }
    Ok(())
}

pub(crate) fn digest(root: &Path, config_only: bool, excluded: &[PathBuf]) -> Result<String, StyleError> {
    let mut files = Vec::new();
    walk_digest(root, root, config_only, excluded, &mut files)?;
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut hasher = Sha256::new();
    for (relative, path) in files {
        let bytes = fs::read(path)?;
        hasher.update((relative.len() as u64).to_le_bytes());
        hasher.update(relative.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

fn walk_digest(
    root: &Path,
    directory: &Path,
    config_only: bool,
    excluded: &[PathBuf],
    files: &mut Vec<(String, PathBuf)>,
) -> Result<(), StyleError> {
    for entry in entries(directory)? {
        let path = entry.path();
        let kind = entry.file_type()?;
        if !config_only && kind.is_file() && excluded.contains(&path) {
            continue;
        }
        if kind.is_symlink() {
            continue;
        }
        let relative = protocol_path(root, &path)?;
        if kind.is_dir() {
            if [".git", "build", "target"].contains(&relative.as_str())
                || (!config_only && relative == ".infra")
                || (config_only
                    && [".infra/.tmp", ".infra/transactions", ".infra/.transactions"].contains(&relative.as_str()))
            {
                continue;
            }
            if !config_only
                || relative == ".infra"
                || relative.starts_with(".infra/")
                || relative == "conan"
                || relative.starts_with("conan/")
            {
                walk_digest(root, &path, config_only, excluded, files)?;
            }
        } else if kind.is_file()
            && (!config_only
                || relative.starts_with(".infra/")
                || ["CMakeLists.txt", "CMakePresets.json", "conanfile.py", "conanfile.txt"]
                    .contains(&relative.as_str())
                || relative.starts_with("conan/"))
        {
            files.push((relative, path));
        }
    }
    Ok(())
}
