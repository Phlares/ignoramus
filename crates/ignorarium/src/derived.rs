//! Typed bodies of the records that carry derived values. One parser per record kind.
use crate::canon::{sha256_hex, to_canonical};
use crate::records::Record;
use crate::spec::model::{Direction, Outcome, SelectionRule};
use crate::units::Cluster;
use ed25519_dalek::SigningKey;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub const ANALYSIS: &str = "analysis";
pub const CHILD_ESTIMATE: &str = "child-estimate";
pub const ESTIMATE: &str = "estimate";
pub const INPUTS: &str = "inputs";
pub const OPENED: &str = "opened";
pub const REGISTER: &str = "register";
pub const RESIZE: &str = "resize";
pub const SELECTION: &str = "selection";
pub const SPAWN_STOP: &str = "spawn-stop";
pub const TRIGGER: &str = "trigger";
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

/// A child's paired values against the reference on the confirmation units, for one outcome:
/// the mean over clusters of each cluster's mean over the units that pair, the cluster and unit
/// counts, and the spec's repeats per unit. A unit pairs when any repeat pairs, as for an
/// `Estimate`. `child` and `parent` are the ids the child ran as (`selected` replaced by the
/// pick). Not an `Estimate`: the "finished" rule counts no child estimate, and no later study
/// reads one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildEstimate {
    pub child: String,
    pub parent: String,
    pub operator: String,
    pub value: Value,
    pub outcome: String,
    /// The confirmation clusters with a unit that pairs, and those units.
    pub clusters: u64,
    pub units: u64,
    /// The mean over clusters of each cluster's mean over its units.
    pub mean: f64,
    pub repeats: u64,
}

impl ChildEstimate {
    /// `None` for another kind of record, or a body that does not parse.
    pub fn of(r: &Record) -> Option<ChildEstimate> {
        body_of(r, CHILD_ESTIMATE)
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
    /// The identity of the study root that the plan's paths were read from (`root_id`). The
    /// record holds no path.
    pub root_id: String,
    /// The tree hash of the payload folder, as copied into the study folder.
    pub payload: String,
    /// Each input present at registration, by path, with the digest of its stored copy.
    pub inputs: Vec<(String, String)>,
    /// The values the plan takes from other studies, as registration found them.
    pub resolved: Vec<Resolved>,
    /// Each deviation the plan declares. A record written before the field existed reads as
    /// none.
    #[serde(default)]
    pub deviations: Vec<PinDeviated>,
    /// The passed determinism check the plan takes instead of running its own; `None` for a
    /// plan that runs its own or declares no determinism, and in a record written before the
    /// field existed. Written as `null`.
    #[serde(default)]
    pub carries: Option<Carried>,
    /// For a root, the members it locks; `None` for any other study, and left out then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<crate::family::Lock>,
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

    /// The identity of the study root `root` in the box of `key`: the hex SHA-256 of a label,
    /// the key's secret bytes and the root's canonical path. Another root, or the same one
    /// moved, gives another identity. Without the key, no reader of a signed record can test
    /// a guessed path against it.
    pub fn root_id(key: &SigningKey, root: &Path) -> std::io::Result<String> {
        let path = std::fs::canonicalize(root)?;
        let mut bytes = b"ignorarium study root\0".to_vec();
        bytes.extend_from_slice(&key.to_bytes());
        bytes.extend_from_slice(path.as_os_str().as_bytes());
        Ok(sha256_hex(&bytes))
    }
}

/// The bytes a study runs on, fixed at its first prepare: each input by path, in spec order,
/// with the digest of its stored copy, or `None` for an input that was absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inputs {
    pub inputs: Vec<(String, Option<String>)>,
}

impl Inputs {
    /// `None` for another kind of record, or a body that does not parse.
    pub fn of(r: &Record) -> Option<Inputs> {
        body_of(r, INPUTS)
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

/// The decision of a re-size study, made once from its first batch: `continue` appends reserve
/// clusters, `stop` runs no reserve row. It holds counts only, never the spread they came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resize {
    /// One per group a claim is tested in, by stratum; `""` is the pooled group.
    pub strata: Vec<ResizeStratum>,
    /// `CONTINUE` or `STOP`.
    pub decision: String,
    /// Why the study stops; `None` when it continues. Written as `null`.
    #[serde(deserialize_with = "Option::deserialize")]
    pub reason: Option<String>,
}

/// One group's counts: its first-batch and reserve clusters, the clusters its claims need
/// (`None` when its reserve cannot reach that count, or the count could not be made), and the
/// reserve clusters it appends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResizeStratum {
    pub stratum: String,
    pub first: u64,
    pub reserve: u64,
    #[serde(deserialize_with = "Option::deserialize")]
    pub needed: Option<u64>,
    pub appended: u64,
}

pub const CONTINUE: &str = "continue";
pub const STOP: &str = "stop";

impl Resize {
    /// `None` for another kind of record, or a body that does not parse.
    pub fn of(r: &Record) -> Option<Resize> {
        body_of(r, RESIZE)
    }

    /// The paths of the reserve clusters that run: for each group, the first `appended` of
    /// `reserve` it holds (the pooled group holds every one). `reserve` is in the order the
    /// re-size appends (`schedule::reserve_order`). None when the study stops.
    pub fn admitted<'a>(&self, reserve: &[&'a Cluster]) -> BTreeSet<&'a str> {
        if self.decision != CONTINUE {
            return BTreeSet::new();
        }
        (self.strata.iter())
            .flat_map(|g| {
                (reserve.iter())
                    .filter(|c| g.stratum.is_empty() || c.stratum == g.stratum)
                    .take(usize::try_from(g.appended).unwrap_or(usize::MAX))
                    .map(|c| c.path.as_str())
            })
            .collect()
    }
}

/// A trigger's bet on the selection data, made once at the selection boundary: its stopped
/// e-value, whether that reached the threshold, and the children it admitted and skipped, each
/// by the id it runs as (`selected` replaced by the pick).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trigger {
    pub id: String,
    pub e_value: f64,
    pub threshold: f64,
    pub fired: bool,
    /// Why the bet could not be read, such as an incomplete selection unit; then `e_value` is
    /// 0. Written as `null`.
    #[serde(deserialize_with = "Option::deserialize")]
    pub reason: Option<String>,
    pub admitted: Vec<String>,
    pub skipped: Vec<Skipped>,
}

/// A child a trigger did not admit, and why: `not-fired`, `same-as-parent` or `budget`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Skipped {
    pub child: String,
    pub reason: String,
}

impl Trigger {
    /// `None` for another kind of record, or a body that does not parse.
    pub fn of(r: &Record) -> Option<Trigger> {
        body_of(r, TRIGGER)
    }
}

/// The children whose remaining rows a run skipped when the budget ran out during child rows,
/// by the id they run as.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnStop {
    pub children: Vec<String>,
    pub reason: String,
}

impl SpawnStop {
    /// `None` for another kind of record, or a body that does not parse.
    pub fn of(r: &Record) -> Option<SpawnStop> {
        body_of(r, SPAWN_STOP)
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

/// A deviation the plan declares, as `register` records it: the study, the reason, the
/// timing, and the parts of that study's code the plan does not pin (a subset of `payload`,
/// `environment`, `inputs`, in that order; empty when it pins them all).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinDeviated {
    pub from: String,
    pub reason: String,
    pub timing: String,
    pub differs: Vec<String>,
}

/// A passed determinism check that a plan takes from a study that ran the same payload in the
/// same environment: the study, and the hash of its `analysis` record, which holds the check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Carried {
    pub study: String,
    pub record: String,
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
    /// The code the estimates describe. A record written before the field existed reads as
    /// an empty one, so its study stays finished.
    #[serde(default)]
    pub instrument: Instrument,
    /// The determinism check of a plan that declares `pins.deterministic`; `None` for one
    /// that does not, and in a record written before the field existed. Written as `null`.
    #[serde(default)]
    pub recheck: Option<Recheck>,
    /// In a study that re-sized, the blinded sd of each claim's first-batch observations in
    /// each group it is tested in; the `resize` record holds counts alone, so the spread is
    /// shown only once the study is over. Left out for a study that did not re-size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resize_sd: Option<Vec<GroupSd>>,
    /// The same over the appended reserve clusters, `sd` `null` for a group that appended
    /// none: with `resize_sd`, the spread of the two stages side by side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appended_sd: Option<Vec<GroupSd>>,
    /// Per mean claim and group, how many units its range clipped. Left out when no claim
    /// is on a mean, and in a record written before the field existed: both read as none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clipped: Vec<GroupClip>,
}

/// The blinded sd (`evidence::power::blinded_sd`) of a claim's cluster observations in one
/// group (`""` pools every cluster); `None` when there is no observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupSd {
    pub hypothesis: String,
    pub stratum: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub sd: Option<f64>,
}

/// Of the units a mean claim's test read in one group (`""` pools every cluster), how many lay
/// below the range's `lo` and above its `hi`, and how many it read. A unit counts once, though
/// its cluster gives one observation per bound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupClip {
    pub hypothesis: String,
    pub stratum: String,
    pub below: u64,
    pub above: u64,
    pub units: u64,
}

/// What the recheck copies of a run found, one copy per condition: `rows` counts the copies
/// compared with their originals; `mismatched` lists the compared copies whose exit or output
/// differed, and `uncompared` the copies not compared (a void row, or another environment or
/// level), each in the order they ran; `carried_from` names the study whose passed check this
/// one takes instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recheck {
    pub rows: u64,
    pub mismatched: Vec<u64>,
    pub uncompared: Vec<u64>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub carried_from: Option<String>,
}

impl Recheck {
    /// Every condition's copy was compared and matched, or the check was carried.
    pub fn passed(&self) -> bool {
        let compared = self.rows > 0 && self.uncompared.is_empty();
        self.mismatched.is_empty() && (compared || self.carried_from.is_some())
    }
}

/// The code a finished run measured with: the payload folder's tree hash, the environment
/// every row ran in (`"host"` or the image's store digest; `""` when the rows name more than
/// one or none), and each model and calibration input present as (path, role, store digest),
/// sorted by path.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instrument {
    pub payload: String,
    pub environment: String,
    pub inputs: Vec<(String, String, String)>,
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
