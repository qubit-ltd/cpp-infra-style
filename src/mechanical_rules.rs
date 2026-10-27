// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Byte and line rules without claims about C++ semantic correctness.

use serde_json::Value;
use serde_json::json;

use crate::config::Loaded;
use crate::scope::Source;
use crate::style_error::StyleError;

pub(crate) fn diagnostic(path: &str, line: usize, rule: &str, message: &str) -> Value {
    json!({"code":format!("STYLE_{}",rule.replace('-',"_").to_uppercase()),"severity":"error","message":message,"project_relative_path":path,"line":line,"column":1,"rule":rule,"suppression_reason":null})
}

pub(crate) fn check(source: &Source, loaded: &Loaded) -> Result<Vec<Value>, StyleError> {
    let mut diagnostics = Vec::new();
    let content = std::str::from_utf8(&source.original).map_err(StyleError::configuration_message)?;
    if loaded.config.rules.iter().any(|r| r == "spdx-header") {
        let header_content = if loaded.policy.allow_bom {
            content.strip_prefix('\u{feff}').unwrap_or(content)
        } else {
            content
        };
        let prefix = header_content.lines().take(8).collect::<Vec<_>>();
        let license = format!("SPDX-License-Identifier: {}", loaded.policy.spdx_license);
        let spdx = prefix
            .iter()
            .any(|line| line.trim_start_matches(['/', '*', ' ']).trim() == license);
        let copyright = prefix
            .iter()
            .any(|line| line.trim_start().starts_with("//") && line.contains(&loaded.policy.copyright));
        if !spdx
            || !copyright
            || (!loaded.policy.allow_bom && content.starts_with('\u{feff}'))
            || content.starts_with("#!")
        {
            diagnostics.push(diagnostic(
                &source.relative,
                1,
                "spdx-header",
                "header must contain the public SPDX identifier and copyright statement",
            ));
        }
    }
    if loaded.config.rules.iter().any(|r| r == "final-newline") && !source.original.ends_with(b"\n") {
        diagnostics.push(diagnostic(
            &source.relative,
            content.lines().count().max(1),
            "final-newline",
            "file must end with a newline",
        ));
    }
    if loaded.config.rules.iter().any(|r| r == "trailing-whitespace") {
        for (index, line) in content.split('\n').enumerate() {
            if line.trim_end_matches('\r').ends_with([' ', '\t']) {
                diagnostics.push(diagnostic(
                    &source.relative,
                    index + 1,
                    "trailing-whitespace",
                    "remove trailing whitespace",
                ));
            }
        }
    }
    Ok(diagnostics)
}

pub(crate) fn exceptions(diagnostics: &mut [Value], loaded: &Loaded) -> Value {
    let mut counts = Vec::new();
    for exception in &loaded.exceptions {
        let mut matches = 0;
        for diagnostic in diagnostics.iter_mut() {
            if diagnostic["rule"] == exception.rule && diagnostic["project_relative_path"] == exception.path {
                diagnostic["suppression_reason"] = json!(exception.reason);
                matches += 1;
            }
        }
        counts.push(json!({"rule":exception.rule,"path":exception.path,"reason":exception.reason,"matched":matches}));
    }
    json!(counts)
}
