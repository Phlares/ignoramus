//! The run schedule: one row per (unit, condition, repeat), written once.
use crate::canon;
use crate::seed::seed;
use crate::spec::model::Spec;
use serde::Serialize;
use std::fmt;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub index: u64,
    pub unit: String,
    pub cluster: String,
    pub stratum: String,
    pub condition: String,
    pub repeat: u32,
    pub shard: u32,
    pub seed_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Schedule {
    pub spec_hash: String,
    pub box_version: String,
    pub rows: Vec<Row>,
}

impl Schedule {
    pub fn new(spec_hash: String, rows: Vec<Row>) -> Schedule {
        Schedule {
            spec_hash,
            box_version: env!("CARGO_PKG_VERSION").to_string(),
            rows,
        }
    }

    /// Canonical JSON plus one trailing LF.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ScheduleError> {
        let value = serde_json::to_value(self).map_err(|e| ScheduleError::Encode(e.to_string()))?;
        let mut bytes =
            canon::to_canonical(&value).map_err(|e| ScheduleError::Encode(e.to_string()))?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

#[derive(Debug)]
pub enum ExpandError {
    /// `design.reference` is not a condition id.
    UnknownReference(String),
}

impl fmt::Display for ExpandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExpandError::UnknownReference(r) => {
                write!(f, "design.reference `{r}` names no condition")
            }
        }
    }
}

impl std::error::Error for ExpandError {}

type UnitKey = (String, String, String);

/// Rows in the order stratum, cluster, unit, then reference condition first,
/// then the other conditions in spec order, then repeat 0..repeats.
/// `units` holds (unit, cluster, stratum); input order does not matter.
pub fn expand(
    spec: &Spec,
    spec_hash: &[u8; 32],
    units: &[UnitKey],
) -> Result<Vec<Row>, ExpandError> {
    let d = &spec.design;
    if !d.conditions.iter().any(|c| c.id == d.reference) {
        return Err(ExpandError::UnknownReference(d.reference.clone()));
    }
    let conditions: Vec<&str> = d
        .conditions
        .iter()
        .filter(|c| c.id == d.reference)
        .chain(d.conditions.iter().filter(|c| c.id != d.reference))
        .map(|c| c.id.as_str())
        .collect();
    let mut sorted: Vec<&UnitKey> = units.iter().collect();
    sorted.sort_by(|a, b| (&a.2, &a.1, &a.0).cmp(&(&b.2, &b.1, &b.0)));
    let mut rows = Vec::new();
    let mut index: u64 = 0;
    for u in sorted {
        for cond in &conditions {
            for repeat in 0..d.repeats {
                rows.push(Row {
                    index,
                    unit: u.0.clone(),
                    cluster: u.1.clone(),
                    stratum: u.2.clone(),
                    condition: (*cond).to_string(),
                    repeat,
                    shard: 0,
                    seed_hex: hex::encode(seed(spec_hash, &format!("row/{index}"))),
                });
                index = index.saturating_add(1);
            }
        }
    }
    Ok(rows)
}

#[derive(Debug)]
pub enum ScheduleError {
    /// `schedule.json` exists with different bytes.
    Differs,
    /// `schedule.json` is a symbolic link.
    Symlink,
    Encode(String),
    Io(std::io::Error),
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScheduleError::Differs => write!(f, "schedule.json exists and differs"),
            ScheduleError::Symlink => write!(f, "schedule.json is a symbolic link"),
            ScheduleError::Encode(e) => write!(f, "schedule encoding failed: {e}"),
            ScheduleError::Io(e) => write!(f, "schedule I/O failed: {e}"),
        }
    }
}

impl std::error::Error for ScheduleError {}

impl From<std::io::Error> for ScheduleError {
    fn from(e: std::io::Error) -> Self {
        ScheduleError::Io(e)
    }
}

/// Write `dir/schedule.json` once. The same bytes again is `Ok`.
pub fn write_once(dir: &Path, s: &Schedule) -> Result<(), ScheduleError> {
    let bytes = s.to_bytes()?;
    let path = dir.join("schedule.json");
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.file_type().is_symlink() => return Err(ScheduleError::Symlink),
        Ok(_) => {
            return if std::fs::read(&path)? == bytes {
                Ok(())
            } else {
                Err(ScheduleError::Differs)
            };
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let tmp = dir.join("schedule.json.tmp");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}
