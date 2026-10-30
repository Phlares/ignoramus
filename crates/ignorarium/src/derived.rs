//! Typed bodies of the records that carry derived values. One parser per record kind.
use crate::records::Record;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ESTIMATE: &str = "estimate";
pub const REGISTER: &str = "register";
pub const SELECTION: &str = "selection";

/// The body of `r` when it is a record of `kind`; `None` for another kind, or a body that
/// does not parse.
fn body_of<T: DeserializeOwned>(r: &Record, kind: &str) -> Option<T> {
    if r.kind != kind {
        return None;
    }
    serde_json::from_value(r.body.clone()).ok()
}

/// A variance measured by a finished study, for a later study to plan with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Estimate {
    pub hypothesis: String,
    pub outcome: String,
    pub key: Value,
    pub key_hash: String,
    pub stratum: String,
    pub clusters: u64,
    pub units: u64,
    pub mean: f64,
    pub var: f64,
    pub icc: Option<f64>,
}

impl Estimate {
    /// `None` for another kind of record, or a body that does not parse.
    pub fn of(r: &Record) -> Option<Estimate> {
        body_of(r, ESTIMATE)
    }
}

/// What `register` records about a plan. A confirmatory plan carries `timestamp = "pending"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Register {
    pub spec_hash: String,
    pub leaf: u64,
    pub log: String,
    pub defaults: Value,
    /// The absolute study root that the plan's paths were read from.
    pub root: String,
    /// The tree hash of the payload folder, as copied into the study folder.
    pub payload: String,
    /// Each input present at registration, by path, with the digest of its stored copy.
    pub inputs: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

impl Register {
    /// `None` for another kind of record, or a body that does not parse.
    pub fn of(r: &Record) -> Option<Register> {
        body_of(r, REGISTER)
    }
}

/// The selection stage's pick: each candidate's mean paired metric over the complete units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub metric: String,
    /// In the order the stage lists them.
    pub candidates: Vec<Candidate>,
    pub selected: String,
    /// Selection units with a paired metric value for every candidate.
    pub complete_units: u64,
    pub incomplete_units: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub id: String,
    pub mean: f64,
}

impl Selection {
    /// `None` for another kind of record, or a body that does not parse.
    pub fn of(r: &Record) -> Option<Selection> {
        body_of(r, SELECTION)
    }
}
