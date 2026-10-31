//! Typed bodies of the records that carry derived values. One parser per record kind.
use crate::canon::{sha256_hex, to_canonical};
use crate::records::Record;
use crate::spec::model::{Direction, Outcome, SelectionRule};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const ANALYSIS: &str = "analysis";
pub const ESTIMATE: &str = "estimate";
pub const REGISTER: &str = "register";
pub const SELECTION: &str = "selection";
pub const VERDICT: &str = "verdict";

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

    /// What makes an estimate distinct: its hypothesis, contrast and stratum.
    pub fn identity(&self) -> (String, String, String) {
        (
            self.hypothesis.clone(),
            self.key_hash.clone(),
            self.stratum.clone(),
        )
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
    /// The values the plan takes from other studies, as registration found them.
    pub resolved: Vec<Resolved>,
    /// The stream slot a confirmatory plan holds; `None` for an exploratory one. Written as
    /// `null`, never left out.
    #[serde(deserialize_with = "Option::deserialize")]
    pub alpha: Option<Alpha>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// A slot of the stream of `log` and the level it gives the study that holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alpha {
    pub log: String,
    pub slot: u64,
    pub level: f64,
}

impl Register {
    /// `None` for another kind of record, or a body that does not parse.
    pub fn of(r: &Record) -> Option<Register> {
        body_of(r, REGISTER)
    }
}

/// The selection stage's pick: each candidate's mean paired metric over the complete units.
/// The record alone shows the pick: the best mean in `direction`, the first listed on a tie.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub metric: String,
    pub rule: SelectionRule,
    /// The metric outcome's direction: which mean is best.
    pub direction: Direction,
    /// In the order the stage lists them.
    pub candidates: Vec<Candidate>,
    pub selected: String,
    /// The complete units, sorted: selection units with a paired metric value for every
    /// candidate. The means are over these units.
    pub units: Vec<String>,
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

/// A value taken from another study: where the plan names it, the study, the hashes of the
/// records it comes from, and the value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resolved {
    pub path: String,
    pub study: String,
    pub records: Vec<String>,
    pub value: Value,
}

/// The first record of a finished run. It states how many records follow it, so a reader can
/// tell a finished study from one whose run stopped after this record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Analysis {
    pub commit: String,
    pub box_version: String,
    /// Estimate records that follow, distinct by hypothesis, key hash and stratum.
    pub estimates: u64,
    /// Verdict records that follow, one per hypothesis.
    pub verdicts: u64,
    /// The stream slot the study's e-value closes; `None` when it holds none. Written as
    /// `null`, never left out.
    #[serde(deserialize_with = "Option::deserialize")]
    pub slot: Option<u64>,
}

impl Analysis {
    /// `None` for another kind of record, or a body that does not parse.
    pub fn of(r: &Record) -> Option<Analysis> {
        body_of(r, ANALYSIS)
    }
}

/// What a finished run found for one hypothesis. `verdict` is `pass`, `fail` or `none`.
/// `basis` names what decided it: `test`, the e-value test of a confirmatory study; `estimate`,
/// the point estimate of an exploratory claim; `none`, a hypothesis with no claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verdict {
    pub hypothesis: String,
    pub verdict: String,
    pub basis: String,
    /// The hypothesis's share of the study's level; a test only.
    #[serde(deserialize_with = "Option::deserialize")]
    pub level: Option<f64>,
    /// The least e-value of its bounds, or 0 when a unit in scope is incomplete or the test
    /// refuses the claim; a test only.
    #[serde(deserialize_with = "Option::deserialize")]
    pub e_value: Option<f64>,
    /// Each bound in each stratum the claim is held to.
    pub bounds: Vec<BoundResult>,
    /// Picked units in scope without a paired value for every term of the contrast.
    pub incomplete_units: u64,
    /// Why a claim failed before its value could decide.
    #[serde(deserialize_with = "Option::deserialize")]
    pub reason: Option<String>,
}

impl Verdict {
    /// `None` for another kind of record, or a body that does not parse.
    pub fn of(r: &Record) -> Option<Verdict> {
        body_of(r, VERDICT)
    }
}

/// One bound of a claim in one stratum (`""` pools every confirmation cluster): `below` or
/// `above` its margin, over the clusters that give an observation. `value` is the bound's
/// e-value in a test and the mean of its observations in an estimate; an estimate with no
/// observation has none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundResult {
    pub stratum: String,
    pub side: String,
    pub margin: f64,
    pub clusters: u64,
    #[serde(deserialize_with = "Option::deserialize")]
    pub value: Option<f64>,
}

/// What a study's records allow a later study to read.
#[derive(Debug, Clone, PartialEq)]
pub enum StudyState {
    NotRegistered,
    NotFinished,
    /// The records after the first analysis record: the ones it promised, and only those
    /// count as the study's results.
    Finished(Vec<Record>),
}

impl StudyState {
    /// `records` is the log of a registered study, or `None` for no registered study. The
    /// study is finished when its first `analysis` record is followed by every estimate and
    /// verdict that record promises, and the stream slot it promises is closed: `closed(log,
    /// slot)` says so for the stream of the log the study registered in. A run that stopped
    /// after the analysis record is not finished: its next run adds what is missing. An error
    /// from `closed` is the stream's.
    pub fn of(
        records: Option<Vec<Record>>,
        closed: impl FnOnce(&str, u64) -> Result<bool, String>,
    ) -> Result<StudyState, String> {
        let Some(records) = records else {
            return Ok(StudyState::NotRegistered);
        };
        let Some(at) = records.iter().position(|r| r.kind == ANALYSIS) else {
            return Ok(StudyState::NotFinished);
        };
        let Some(promised) = Analysis::of(&records[at]) else {
            return Ok(StudyState::NotFinished);
        };
        let after = &records[at + 1..];
        let estimates: BTreeSet<(String, String, String)> = after
            .iter()
            .filter_map(Estimate::of)
            .map(|e| e.identity())
            .collect();
        let verdicts: BTreeSet<String> = after
            .iter()
            .filter_map(Verdict::of)
            .map(|v| v.hypothesis)
            .collect();
        if (estimates.len() as u64) < promised.estimates
            || (verdicts.len() as u64) < promised.verdicts
        {
            return Ok(StudyState::NotFinished);
        }
        if let Some(slot) = promised.slot {
            let log = records.iter().find_map(Register::of).map(|r| r.log);
            match log {
                Some(log) if closed(&log, slot)? => {}
                _ => return Ok(StudyState::NotFinished),
            }
        }
        Ok(StudyState::Finished(after.to_vec()))
    }
}

/// The key of a contrast's estimate: canonical `{"outcome", "measure", "paired", "repeats",
/// "reference", "contrast"}`, the reference as `[condition id, params]` (a paired value is
/// taken against it) and each term as `[condition id, weight, params]`, sorted by id (then
/// weight); and the SHA-256 hex of its canonical JSON. A follow-on study finds a finished
/// study's estimate by this hash.
pub fn contrast_key(
    outcome: &Outcome,
    repeats: u64,
    reference: &(String, Value),
    terms: &[(String, f64, Value)],
) -> (Value, String) {
    let mut terms = terms.to_vec();
    terms.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let key = json!({
        "outcome": outcome.id,
        "measure": outcome.measure,
        "paired": outcome.paired,
        "repeats": repeats,
        "reference": [reference.0, reference.1],
        "contrast": terms
            .into_iter()
            .map(|(id, weight, params)| json!([id, weight, params]))
            .collect::<Vec<_>>(),
    });
    let bytes = to_canonical(&key).expect("a key built from finite numbers serializes");
    let hash = sha256_hex(&bytes);
    (key, hash)
}
