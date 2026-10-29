//! Diagnostics and their two renderings.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub path: String,
    pub problem: String,
    pub instruction: String,
    /// `docs/rules/<code>.md`.
    pub doc: String,
    pub sources: Vec<String>,
}

/// One block per diagnostic, a blank line between blocks.
pub fn render_human(diags: &[Diagnostic]) -> String {
    diags
        .iter()
        .map(|d| {
            format!(
                "{}[{}] {}: {}\n  fix: {}\n  see: {}\n",
                d.severity.as_str(),
                d.code,
                d.path,
                d.problem,
                d.instruction,
                d.doc
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_json(diags: &[Diagnostic]) -> String {
    // Strings and enums only: serialization cannot fail.
    serde_json::to_string_pretty(diags).unwrap_or_default()
}
