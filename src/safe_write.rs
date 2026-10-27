// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Optimistic source replacement without overwriting intervening edits.

use std::fs;
use std::io::Write;

use tempfile::NamedTempFile;

use crate::scope::Source;
use crate::style_error::StyleError;

/// Replace one file only while its identity, bytes and permissions still match.
/// The temporary output is created on the source filesystem for atomic rename.
pub(crate) fn replace(source: &Source, formatted: &[u8]) -> Result<(), StyleError> {
    let parent = source
        .path
        .parent()
        .ok_or_else(|| StyleError::Configuration("source has no parent".into()))?;
    if fs::canonicalize(&source.path)? != source.path {
        return Err(StyleError::ConcurrentModification(source.relative.clone()));
    }
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(formatted)?;
    temporary.as_file().set_permissions(source.permissions.clone())?;
    temporary.as_file().sync_all()?;
    let metadata = fs::symlink_metadata(&source.path)?;
    if fs::canonicalize(&source.path)? != source.path
        || metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.permissions() != source.permissions
        || fs::read(&source.path)? != source.original
    {
        return Err(StyleError::ConcurrentModification(source.relative.clone()));
    }
    temporary
        .persist(&source.path)
        .map_err(|e| StyleError::Infrastructure(format!("replace {}: {}", source.relative, e.error)))?;
    Ok(())
}
