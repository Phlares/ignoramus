//! Typed bodies of the records that carry derived values. One parser per record kind.
use crate::records::Record;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ESTIMATE: &str = "estimate";

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
        if r.kind != ESTIMATE {
            return None;
        }
        serde_json::from_value(r.body.clone()).ok()
    }
}
