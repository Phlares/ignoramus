//! The run schedule: one row per (unit, condition, repeat), written once.
use crate::canon;
use crate::files::{self, WriteOnceError};
use crate::seed::seed;
use crate::spec::model::{Role, Spec};
use crate::units::{self, Cluster};
use serde::Serialize;
use std::fmt;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub index: u64,
    /// `"selection"` or `"confirmation"`: the role of the row's cluster.
    pub stage: String,
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
    /// Stored as `"sha256:<hex>"`, the form of `RawSpec.hash`.
    pub fn new(spec_hash: &[u8; 32], rows: Vec<Row>) -> Schedule {
        Schedule {
            spec_hash: format!("sha256:{}", hex::encode(spec_hash)),
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

/// The most rows a schedule may hold. `check` builds the whole schedule to count its rows,
/// so a rule refuses a larger one first (E-UNT-002).
pub const ROW_CAP: u64 = 100_000;

/// The stage a cluster of `role` runs in, and the conditions that stage runs besides the
/// reference. A selection cluster has a stage only when the spec declares one.
fn stage(spec: &Spec, role: Role) -> Option<(&'static str, &[String])> {
    match role {
        Role::Selection => spec
            .design
            .selection
            .as_ref()
            .map(|s| ("selection", &s.conditions[..])),
        Role::Confirmation => Some(("confirmation", &spec.design.confirmation.conditions[..])),
        Role::Calibration | Role::Model => None,
    }
}

/// Rows in the order stage (selection first), stratum, cluster path, then each picked unit,
/// then the reference condition, then the stage's conditions in the order it lists them, then
/// repeat 0..repeats. The order of `clusters` does not matter.
pub fn expand(
    spec: &Spec,
    spec_hash: &[u8; 32],
    clusters: &[Cluster],
) -> Result<Vec<Row>, ExpandError> {
    let d = &spec.design;
    if !d.conditions.iter().any(|c| c.id == d.reference) {
        return Err(ExpandError::UnknownReference(d.reference.clone()));
    }
    let mut staged: Vec<(&str, &[String], &Cluster)> = clusters
        .iter()
        .filter_map(|c| stage(spec, c.role).map(|(name, conds)| (name, conds, c)))
        .collect();
    staged.sort_by_key(|(_, _, c)| (c.role == Role::Confirmation, &c.stratum, &c.path));
    let mut rows = Vec::new();
    let mut index: u64 = 0;
    for (name, conditions, c) in staged {
        let conditions = std::iter::once(&d.reference).chain(conditions);
        for &i in &c.picked {
            for cond in conditions.clone() {
                for repeat in 0..d.repeats {
                    rows.push(Row {
                        index,
                        stage: name.to_string(),
                        unit: units::key(&c.path, i),
                        cluster: c.path.clone(),
                        stratum: c.stratum.clone(),
                        condition: cond.clone(),
                        repeat,
                        shard: 0,
                        seed_hex: hex::encode(seed(spec_hash, &format!("row/{index}"))),
                    });
                    index = index.saturating_add(1);
                }
            }
        }
    }
    Ok(rows)
}

/// The schedule `run` writes for `spec`: its clusters' picked units, expanded.
pub fn rows(spec: &Spec, spec_hash: &[u8; 32]) -> Result<Vec<Row>, ExpandError> {
    expand(spec, spec_hash, &units::clusters(spec, spec_hash))
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

impl From<WriteOnceError> for ScheduleError {
    fn from(e: WriteOnceError) -> Self {
        match e {
            WriteOnceError::Differs => ScheduleError::Differs,
            WriteOnceError::Link => ScheduleError::Symlink,
            WriteOnceError::Io(e) => ScheduleError::Io(e),
        }
    }
}

/// Write `dir/schedule.json` once. The same bytes again is `Ok`.
pub fn write_once(dir: &Path, s: &Schedule) -> Result<(), ScheduleError> {
    files::write_once(&dir.join("schedule.json"), &s.to_bytes()?, 0o644)?;
    Ok(())
}
