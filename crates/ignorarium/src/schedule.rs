//! The run schedule: one row per (unit, condition, repeat), written once.
use crate::canon;
use crate::files::{self, WriteOnceError};
use crate::seed::seed;
use crate::spec::{
    self,
    model::{Param, Pick, Role, Scalar, Spec},
};
use crate::units::{self, Cluster};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

/// The stage names a row carries: the roles of the clusters each stage runs on.
pub const SELECTION_STAGE: &str = "selection";
pub const CONFIRMATION_STAGE: &str = "confirmation";
/// Reserve clusters run the confirmation stage's conditions, after every confirmation row.
pub const RESERVE_STAGE: &str = "reserve";
/// Each declared child on the confirmation clusters, after every reserve row. A child row runs
/// only when a trigger admits its child.
pub const SPAWN_STAGE: &str = "spawn";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub index: u64,
    /// `SELECTION_STAGE`, `CONFIRMATION_STAGE`, `RESERVE_STAGE` or `SPAWN_STAGE`.
    pub stage: String,
    pub unit: String,
    pub cluster: String,
    pub stratum: String,
    pub condition: String,
    pub repeat: u32,
    pub shard: u32,
    pub seed_hex: String,
    /// The index of the row this one repeats, for a recheck copy. Left out of the schedule
    /// file otherwise, so a schedule without copies keeps its bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recheck_of: Option<u64>,
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
/// reference. A selection cluster has a stage when the spec declares one. Without one, a spec
/// with a declared trigger runs each condition a trigger's contrast names on it, in the order
/// `design.conditions` lists them.
fn stage(spec: &Spec, role: Role) -> Option<(&'static str, Vec<&str>)> {
    fn names(list: &[String]) -> Vec<&str> {
        list.iter().map(String::as_str).collect()
    }
    let d = &spec.design;
    match role {
        Role::Selection => match &d.selection {
            Some(s) => Some((SELECTION_STAGE, names(&s.conditions))),
            None if spec.mutations.declared().next().is_some() => {
                let watched =
                    |id: &&str| (spec.mutations.declared()).any(|t| t.contrast.contains_key(*id));
                let ids = (d.conditions.iter())
                    .map(|c| c.id.as_str())
                    .filter(|id| *id != d.reference);
                Some((SELECTION_STAGE, ids.filter(watched).collect()))
            }
            None => None,
        },
        Role::Confirmation => Some((CONFIRMATION_STAGE, names(&d.confirmation.conditions))),
        Role::Reserve => Some((RESERVE_STAGE, names(&d.confirmation.conditions))),
        Role::Calibration | Role::Model => None,
    }
}

/// Rows in the order stage (selection, confirmation, reserve), stratum, cluster path (a
/// reserve cluster's input pin instead), then each picked unit, then the reference condition,
/// then the stage's conditions in the order it lists them, then repeat 0..repeats. Then the
/// spawn stage: each child `children` lists, on the confirmation clusters in their order,
/// each picked unit, then repeat. The order of `clusters` does not matter.
pub fn expand(
    spec: &Spec,
    spec_hash: &[u8; 32],
    clusters: &[Cluster],
) -> Result<Vec<Row>, ExpandError> {
    let d = &spec.design;
    if !d.conditions.iter().any(|c| c.id == d.reference) {
        return Err(ExpandError::UnknownReference(d.reference.clone()));
    }
    // (stage, its conditions, cluster), in the order the rows run.
    let mut blocks: Vec<(&str, Vec<&str>, &Cluster)> = clusters
        .iter()
        .filter_map(|c| {
            let (name, conds) = stage(spec, c.role)?;
            Some((name, [vec![d.reference.as_str()], conds].concat(), c))
        })
        .collect();
    let pins = pins(spec);
    blocks.sort_by_key(|(_, _, c)| place(&pins, c));
    let confirmation: Vec<&Cluster> = (blocks.iter())
        .filter(|(_, _, c)| c.role == Role::Confirmation)
        .map(|(_, _, c)| *c)
        .collect();
    let children = children(spec);
    for child in &children {
        let spawned = confirmation
            .iter()
            .map(|c| (SPAWN_STAGE, vec![&child.id[..]], *c));
        blocks.extend(spawned);
    }
    let mut rows = Vec::new();
    let mut index: u64 = 0;
    for (name, conditions, c) in blocks {
        for &i in &c.picked {
            for cond in &conditions {
                for repeat in 0..d.repeats {
                    rows.push(Row {
                        index,
                        stage: name.to_string(),
                        unit: units::key(&c.path, i),
                        cluster: c.path.clone(),
                        stratum: c.stratum.clone(),
                        condition: cond.to_string(),
                        repeat,
                        shard: 0,
                        seed_hex: hex::encode(seed(spec_hash, &format!("row/{index}"))),
                        recheck_of: None,
                    });
                    index = index.saturating_add(1);
                }
            }
        }
    }
    Ok(rows)
}

/// A condition a trigger spawns: `parent` with its param `operator` set to `value`.
#[derive(Debug, Clone, PartialEq)]
pub struct Child {
    /// `<parent>~<operator>=<value>`, the value as `shown` writes it. The id is the child's
    /// identity: two declarations with one id are one child.
    pub id: String,
    /// The condition the operator acts on, or `selected`, which the pick replaces at run.
    pub parent: String,
    pub operator: String,
    /// As first declared.
    pub value: Scalar,
    /// The triggers that declare it, in spec order.
    pub triggers: Vec<String>,
}

/// A spawn value as a child id writes it. Equal numbers show the same: 64 and 64.0 as `64`,
/// -0.0 as `0`. A float shows as the shortest decimal that reads back to it.
pub fn shown(v: &Scalar) -> String {
    match v {
        Scalar::Int(i) => i.to_string(),
        Scalar::Float(f) if *f == 0.0 => "0".into(),
        Scalar::Float(f) => f.to_string(),
        Scalar::Text(t) => t.clone(),
    }
}

/// The id of the child of `parent` with `operator` set to `value`.
pub fn child_id(parent: &str, operator: &str, value: &Scalar) -> String {
    format!("{parent}~{operator}={}", shown(value))
}

/// The declared derivative set: each child of each declared trigger, in trigger order, then
/// spawns order, then values order, once. A spawn whose operator is not declared spawns
/// nothing, and a value the parent's param holds is no child. A child of `selected` is
/// compared with the pick's params at run.
pub fn children(spec: &Spec) -> Vec<Child> {
    let m = &spec.mutations;
    let mut out: Vec<Child> = Vec::new();
    for t in m.declared() {
        for s in &t.spawns {
            let Some(op) = m.operators.iter().find(|o| o.name == s.operator) else {
                continue;
            };
            let parent = spec.design.conditions.iter().find(|c| c.id == op.acts_on);
            let held = match parent.and_then(|c| c.params.get(&op.name)) {
                Some(Param::Value(v)) => Some(shown(v)),
                _ => None,
            };
            for value in &s.values {
                if held.as_deref() == Some(&shown(value)[..]) {
                    continue;
                }
                let id = child_id(&op.acts_on, &op.name, value);
                match out.iter_mut().find(|c| c.id == id) {
                    Some(c) if !c.triggers.contains(&t.id) => c.triggers.push(t.id.clone()),
                    Some(_) => {}
                    None => out.push(Child {
                        id,
                        parent: op.acts_on.clone(),
                        operator: op.name.clone(),
                        value: value.clone(),
                        triggers: vec![t.id.clone()],
                    }),
                }
            }
        }
    }
    out
}

/// Each input's pin by path; the first input with a path names it.
fn pins(spec: &Spec) -> BTreeMap<&str, &str> {
    (spec.inputs.iter().rev())
        .map(|i| (&i.path[..], &i.sha256[..]))
        .collect()
}

/// The sort key of a cluster's rows: its stage, its stratum, its path (a reserve cluster's
/// input pin instead), then its path. A reserve cluster goes by its pin, so a re-size admits
/// the same clusters whatever the files are named.
fn place<'a>(pins: &BTreeMap<&'a str, &'a str>, c: &'a Cluster) -> (u8, &'a str, &'a str, &'a str) {
    let (rank, key) = match c.role {
        Role::Selection => (0, &c.path[..]),
        Role::Reserve => (2, pins.get(&c.path[..]).copied().unwrap_or("")),
        _ => (1, &c.path[..]),
    };
    (rank, &c.stratum[..], key, &c.path[..])
}

/// The reserve clusters of `clusters`, in the order the schedule runs them and a re-size
/// appends them: by stratum, then pin, then path.
pub fn reserve_order<'a>(spec: &'a Spec, clusters: &'a [Cluster]) -> Vec<&'a Cluster> {
    let pins = pins(spec);
    let mut reserve: Vec<&Cluster> = (clusters.iter())
        .filter(|c| c.role == Role::Reserve)
        .collect();
    reserve.sort_by_key(|c| place(&pins, c));
    reserve
}

/// The typed spec and its schedule for the defaulted `value` with spec hash `hash`; `None`
/// when a value that passes every rule still does not fit the typed model or build a
/// schedule, which `rules::schema_fault` reports. `carried`: the plan takes its determinism
/// check from another study, so its schedule holds no recheck rows.
pub fn of_value(value: &Value, hash: &str, carried: bool) -> Option<(Spec, Vec<Row>)> {
    let s = Spec::from_value(value).ok()?;
    let hash = spec::hash_bytes(hash)?;
    let rows = rows(&s, &hash, s.pins.deterministic && !carried).ok()?;
    Some((s, rows))
}

/// The schedule `run` writes for `spec`: its clusters' picked units, expanded, and with
/// `rechecks`, a recheck copy per condition (`with_rechecks`).
pub fn rows(spec: &Spec, spec_hash: &[u8; 32], rechecks: bool) -> Result<Vec<Row>, ExpandError> {
    let rows = expand(spec, spec_hash, &units::clusters(spec, spec_hash))?;
    Ok(if rechecks {
        with_rechecks(rows, spec_hash)
    } else {
        rows
    })
}

/// `rows` with one copy of a row of each condition id, which repeats that row's job so the
/// two outputs can be compared. A reserve row has no copy. The j-th condition id, in order of
/// first appearance, has its row drawn from `seed(spec_hash, "recheck/<condition>")`; the copy
/// takes index `rows.len() + j` and `recheck_of` the original's index, keeps every other
/// field, and runs right after the original.
pub fn with_rechecks(rows: Vec<Row>, spec_hash: &[u8; 32]) -> Vec<Row> {
    // Each condition id with the positions of its rows, in order of first appearance.
    let mut conditions: Vec<(&str, Vec<usize>)> = Vec::new();
    let first_batch = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.stage != RESERVE_STAGE);
    for (at, r) in first_batch {
        match conditions.iter_mut().find(|(c, _)| *c == r.condition) {
            Some((_, of)) => of.push(at),
            None => conditions.push((&r.condition, vec![at])),
        }
    }
    // Each copy by the position of its original.
    let mut copies: BTreeMap<usize, Row> = BTreeMap::new();
    for (j, (condition, of)) in conditions.iter().enumerate() {
        let draw = seed(spec_hash, &format!("recheck/{condition}"));
        let n = u32::try_from(of.len()).unwrap_or(u32::MAX);
        for i in units::pick(1, n, Pick::Seeded, draw) {
            let at = of[i as usize];
            let copy = Row {
                index: (rows.len() + j) as u64,
                recheck_of: Some(rows[at].index),
                ..rows[at].clone()
            };
            copies.insert(at, copy);
        }
    }
    let mut out = Vec::with_capacity(rows.len() + copies.len());
    for (at, r) in rows.into_iter().enumerate() {
        out.push(r);
        out.extend(copies.remove(&at));
    }
    out
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
