//! `deviations.toml`: the deviations a person writes down by hand. No key signs this file, so
//! the card marks its rows unsigned and names the file's hash.
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FILE: &str = "deviations.toml";

/// When a deviation happened (Willroth's five stages).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Timing {
    BeforeCollection,
    DuringCollection,
    AfterCollectionBeforeAccess,
    AfterAccessBeforeResults,
    AfterResultsKnown,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deviation {
    #[serde(rename = "type")]
    pub kind: String,
    pub reason: String,
    pub timing: Timing,
    pub original: String,
    pub impact: String,
    /// A line pointer or a file hash.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unregistered {
    pub step: String,
    pub reason: String,
    pub timing: Timing,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Deviations {
    pub deviation: Vec<Deviation>,
    pub unregistered: Vec<Unregistered>,
    /// SHA-256 of the file's bytes.
    pub sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    #[serde(default)]
    deviation: Vec<Deviation>,
    #[serde(default)]
    unregistered: Vec<Unregistered>,
}

/// The deviations file of the study folder `study_dir`; `None` when there is none.
pub fn load(study_dir: &Path) -> Result<Option<Deviations>, String> {
    let path = study_dir.join(FILE);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{FILE}: {e}")),
    };
    let text = std::str::from_utf8(&bytes).map_err(|e| format!("{FILE}: {e}"))?;
    let raw: Raw = toml::from_str(text).map_err(|e| format!("{FILE}: {e}"))?;
    Ok(Some(Deviations {
        deviation: raw.deviation,
        unregistered: raw.unregistered,
        sha256: crate::canon::sha256_hex(&bytes),
    }))
}
