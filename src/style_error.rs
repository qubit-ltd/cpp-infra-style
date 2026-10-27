// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Typed failures and their stable CLI exit classes.

use std::fmt::Display;

use thiserror::Error;

/// Errors distinguish invalid input, infrastructure failures, and cancellation.
#[derive(Debug, Error)]
pub enum StyleError {
    /// Configuration, protocol, or unsupported tool version.
    #[error("{0}")]
    Configuration(String),
    /// Filesystem or subprocess failure, retaining contextual information.
    #[error("{0}")]
    Infrastructure(String),
    /// A source file changed after its original bytes were captured.
    #[error("concurrent modification detected: {0}")]
    ConcurrentModification(String),
    /// The user interrupted the operation.
    #[error("operation cancelled")]
    Cancelled,
}

impl StyleError {
    /// Preserve a lower-level error's message while classifying invalid input.
    pub(crate) fn configuration_message(error: impl Display) -> Self {
        Self::Configuration(error.to_string())
    }

    /// Preserve a lower-level error's message for infrastructure failures.
    pub(crate) fn infrastructure_message(error: impl Display) -> Self {
        Self::Infrastructure(error.to_string())
    }
    /// Return the shared process exit code for this error class.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Configuration(_) | Self::ConcurrentModification(_) => 2,
            Self::Infrastructure(_) => 3,
            Self::Cancelled => 130,
        }
    }
}

impl From<std::io::Error> for StyleError {
    fn from(error: std::io::Error) -> Self {
        Self::infrastructure_message(error)
    }
}
