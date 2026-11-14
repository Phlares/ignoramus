//! The deviation files of a study folder. `deviations.jsonl` is written by the box (run
//! control); `deviations.toml` is written by a person, and no key signs it. Both use the
//! vocabulary of `control`: `Timing` and `DeviationRow`.
use crate::control::{DeviationRow, Timing};
use serde::{Deserialize, Deserializer};
use std::path::Path;

pub const FILE: &str = "deviations.toml";
pub const RECORDED: &str = "deviations.jsonl";

/// A `[[deviation]]`: a control `DeviationRow` and where the person got it from.
#[derive(Debug, Clone, PartialEq)]
pub struct Deviation {
    pub row: DeviationRow,
    /// A line pointer or a file hash.
    pub source: String,
}

impl<'de> Deserialize<'de> for Deviation {
    /// `source` comes out first so that `DeviationRow` keeps refusing every other unknown field.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let mut table = toml::Table::deserialize(d)?;
        let source = (table.remove("source").and_then(|v| v.try_into().ok()))
            .ok_or_else(|| D::Error::missing_field("source"))?;
        let row = DeviationRow::deserialize(table).map_err(D::Error::custom)?;
        Ok(Deviation { row, source })
    }
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

/// The bytes of `name` in `dir`, and as text; `None` when there is no such file.
fn read(dir: &Path, name: &str) -> Result<Option<(String, Vec<u8>)>, String> {
    let bytes = match std::fs::read(dir.join(name)) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{name}: {e}")),
    };
    let text = String::from_utf8(bytes.clone()).map_err(|e| format!("{name}: {e}"))?;
    Ok(Some((text, bytes)))
}

/// The deviations file of the study folder `study_dir`; `None` when there is none.
pub fn load(study_dir: &Path) -> Result<Option<Deviations>, String> {
    let Some((text, bytes)) = read(study_dir, FILE)? else {
        return Ok(None);
    };
    let raw: Raw = toml::from_str(&text).map_err(|e| format!("{FILE}: {e}"))?;
    Ok(Some(Deviations {
        deviation: raw.deviation,
        unregistered: raw.unregistered,
        sha256: crate::canon::sha256_hex(&bytes),
    }))
}

/// The rows the box wrote to `deviations.jsonl`, with the file's SHA-256; `None` when there is
/// no such file.
pub fn recorded(study_dir: &Path) -> Result<Option<(Vec<DeviationRow>, String)>, String> {
    let Some((text, bytes)) = read(study_dir, RECORDED)? else {
        return Ok(None);
    };
    let rows = (text.lines().enumerate())
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| {
            serde_json::from_str(l).map_err(|e| format!("{RECORDED}: line {}: {e}", i + 1))
        })
        .collect::<Result<_, _>>()?;
    Ok(Some((rows, crate::canon::sha256_hex(&bytes))))
}
