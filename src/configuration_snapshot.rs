// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Stable identity and consumed-byte fingerprints for active configuration.

use std::io::Read;
use std::path::Path;
use std::path::PathBuf;

use same_file::Handle;

use crate::config::configuration_input;
use crate::scope::hash;
use crate::style_error::StyleError;

pub(crate) struct ConfigurationSnapshot {
    path: PathBuf,
    relative: String,
    identity: Handle,
    sha256: String,
}

impl ConfigurationSnapshot {
    /// Read tracked configuration through the retained identity handle. Return
    /// the exact consumed bytes; unsafe paths, changed inputs and IO failures
    /// prevent construction of an accepted snapshot.
    pub fn capture(project: &Path, relative: &str) -> Result<(Self, Vec<u8>), StyleError> {
        let path = configuration_input(project, relative)?;
        let mut identity = Handle::from_path(&path)?;
        let mut bytes = Vec::new();
        identity.as_file_mut().read_to_end(&mut bytes)?;
        let snapshot = Self {
            path,
            relative: relative.into(),
            identity,
            sha256: hash(&bytes),
        };
        snapshot.verify(project)?;
        Ok((snapshot, bytes))
    }

    /// Reopen the declared input and compare its canonical location, underlying
    /// file identity and content SHA. Any change invalidates the loaded policy.
    pub fn verify(&self, project: &Path) -> Result<(), StyleError> {
        let changed = || {
            StyleError::Configuration(format!(
                "configuration changed during style execution: {}",
                self.relative
            ))
        };
        let path = configuration_input(project, &self.relative).map_err(|_| changed())?;
        let mut current = Handle::from_path(&path).map_err(|_| changed())?;
        if path != self.path || current != self.identity {
            return Err(changed());
        }
        let mut bytes = Vec::new();
        current.as_file_mut().read_to_end(&mut bytes).map_err(|_| changed())?;
        if hash(&bytes) != self.sha256 {
            return Err(changed());
        }
        Ok(())
    }

    pub fn matches_path(&self, path: &Path) -> bool {
        self.path == path
    }
}
