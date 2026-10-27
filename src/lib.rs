// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Deterministic C++ style checking.

mod cli;
mod config;
mod configuration_snapshot;
mod engine;
mod formatter;
mod mechanical_rules;
mod report;
mod safe_write;
mod scope;
mod source_exclusions;
mod style_error;

#[cfg(test)]
mod tests;

pub use cli::run;
pub use style_error::StyleError;
