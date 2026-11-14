//! The card's sections (design 2.1), each built from the verified log and the registered plan.
//! Files of the study folder that no key signs enter only by path and SHA-256.
use super::Interval;
use crate::analysis::{IntervalResult, claim_strata};
use crate::canon::sha256_file;
use crate::derived::{
    Analysis, Estimate, GroupClip, Inputs, Instrument, Register, Resolved, Trigger, Verdict,
};
use crate::files::optional;
use crate::records::{Log, Record};
use crate::rules::{Curve, CurvePoint};
use crate::runner::Tally;
use crate::spec::model::{Budget, Claim, Mode, Paired, Spec, StatisticSpec};
use crate::telemetry::Totals;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// What a renderer prints for a field no record holds.
pub const NOT_IN_RECORDS: &str = "not in the records";
/// What a renderer prints for a field that has no value in this study.
pub const NOT_APPLICABLE: &str = "not applicable";
/// The fixed line under the list of runs.
pub const UNRECORDED: &str = "Executions recorded nowhere cannot be listed.";
/// How a row's seed derives from the plan hash (`seed::row_hex`).
pub const ROW_SEED: &str = "SHA-256(plan hash bytes || \"row/<index>\"); a recheck copy keeps the seed of the row it repeats";
/// Only the level 2 backend runs an agent beside the payload.
const AGENT_LEVEL: u8 = 2;
/// The claim label line until the box computes labels (audit G1).
pub const LABEL_NOT_COMPUTED: &str = "label not computed";
/// The results note of a quantile claim (ruling R8).
pub const NO_QUANTILE_INTERVAL: &str = "the interval is not built for quantile claims";
/// The study-folder files the card lists by hash, in order.
pub const LISTED: [&str; 5] = [
    "statement.md",
    "gaps.md",
    "decisions.md",
    crate::deviations::FILE,
    super::recipe::CITATION_FILE,
];
/// The run log, which no key signs.
pub const EVENTS: &str = "events.jsonl";
const PENDING: &str = "pending";

/// A file of the study folder, by its path in the folder and its SHA-256. Never parsed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outside {
    pub path: String,
    pub sha256: String,
}

impl Outside {
    /// File `name` of the study folder `dir`; `None` when there is none.
    pub fn of(dir: &Path, name: &str) -> Result<Option<Outside>, String> {
        let hash = optional(sha256_file(&dir.join(name))).map_err(|e| format!("{name}: {e}"))?;
        Ok(hash.map(|sha256| Outside {
            path: name.into(),
            sha256,
        }))
    }
}

/// A card value: from the records, missing from them, or with no value in this study.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Field<T> {
    Recorded {
        value: T,
    },
    /// `link` is the study-folder file that holds the value, when there is one.
    Missing {
        link: Option<Outside>,
    },
    NotApplicable,
}

impl<T> Field<T> {
    pub(crate) fn of(value: Option<T>, link: Option<&Outside>) -> Field<T> {
        match value {
            Some(value) => Field::Recorded { value },
            None => Field::Missing {
                link: link.cloned(),
            },
        }
    }

    /// The value as `show` prints it, `NOT_IN_RECORDS` or `NOT_APPLICABLE`.
    pub fn shown(&self, show: impl FnOnce(&T) -> String) -> String {
        match self {
            Field::Recorded { value } => show(value),
            Field::Missing { .. } => NOT_IN_RECORDS.into(),
            Field::NotApplicable => NOT_APPLICABLE.into(),
        }
    }
}

/// A `LISTED` file; `sha256` is `None` when the study folder does not hold it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Listed {
    pub path: String,
    pub sha256: Option<String>,
}

/// Every `LISTED` file, and the path of the author slots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutsideFiles {
    pub files: Vec<Listed>,
    pub readings: String,
}

pub fn outside(study_dir: &Path) -> Result<OutsideFiles, String> {
    let mut files = vec![];
    for name in LISTED {
        files.push(Listed {
            path: name.into(),
            sha256: Outside::of(study_dir, name)?.map(|o| o.sha256),
        });
    }
    Ok(OutsideFiles {
        files,
        readings: super::readings::FILE.into(),
    })
}

/// The first estimate of each hypothesis, contrast and stratum in `after`, as `report`
/// counts them.
fn estimates(after: &[Record]) -> Vec<Estimate> {
    let mut seen = BTreeSet::new();
    (after.iter().filter_map(Estimate::of))
        .filter(|e| seen.insert(e.identity()))
        .collect()
}

/// The first verdict of each hypothesis in `records`; a later one is a repeat.
pub fn first_verdicts(records: &[Record]) -> Vec<Verdict> {
    let mut seen = BTreeSet::new();
    (records.iter().filter_map(Verdict::of))
        .filter(|v| seen.insert(v.hypothesis.clone()))
        .collect()
}

fn rows(log: &Log) -> impl Iterator<Item = &Record> {
    log.records.iter().filter(|r| r.kind == "row")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub mode: Mode,
    pub blind: bool,
    pub spec_hash: String,
    /// The time on the register record, from the box's clock.
    pub registered_at: String,
    /// An outside time stamp of the registration; a confirmatory plan waits for one.
    pub timestamp: Field<String>,
}

/// The header of a study whose register record `own` was written at `registered_at`.
pub fn header(own: &Register, registered_at: &str, spec: &Spec) -> Header {
    Header {
        mode: spec.study.mode,
        blind: spec.access.blind,
        spec_hash: own.spec_hash.clone(),
        registered_at: registered_at.into(),
        timestamp: Field::of(own.timestamp.clone().filter(|t| t != PENDING), None),
    }
}

/// One registered hypothesis: its claim, and the e-value its test must reach.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimLine {
    pub hypothesis: String,
    pub statement: String,
    pub outcome: String,
    /// Empty: every confirmation cluster as one group.
    pub strata: Vec<String>,
    pub claim: Option<Claim>,
    pub statistic: Option<StatisticSpec>,
    pub level: Option<f64>,
    /// 1 / `level`: the e-value the test must reach in each stratum.
    pub pass_e_value: Option<f64>,
}

pub fn claims(after: &[Record], spec: &Spec) -> Vec<ClaimLine> {
    let levels: BTreeMap<String, Option<f64>> = (first_verdicts(after).into_iter())
        .map(|v| (v.hypothesis, v.level))
        .collect();
    (spec.question.hypotheses.iter())
        .map(|h| {
            let level = levels.get(&h.id).copied().flatten();
            ClaimLine {
                hypothesis: h.id.clone(),
                statement: h.statement.clone(),
                outcome: h.outcome.clone(),
                strata: h.strata.clone(),
                claim: h.claim,
                statistic: h.statistic,
                level,
                pass_e_value: level.map(|l| 1.0 / l),
            }
        })
        .collect()
}

/// A verdict as the card words it: `pass` is shown, `fail` is not shown, `none` makes no claim.
pub fn display_word(verdict: &str) -> &str {
    match verdict {
        "pass" => "shown",
        "fail" => "not shown",
        "none" => "no claim",
        other => other,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerdictLine {
    pub hypothesis: String,
    pub display: String,
    pub label: String,
}

pub fn verdicts(after: &[Record]) -> Vec<VerdictLine> {
    (first_verdicts(after).into_iter())
        .map(|v| VerdictLine {
            display: display_word(&v.verdict).into(),
            hypothesis: v.hypothesis,
            label: LABEL_NOT_COMPUTED.into(),
        })
        .collect()
}

/// One estimate record of a hypothesis in one group (`""` pools every cluster).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultRow {
    pub hypothesis: String,
    pub stratum: String,
    pub outcome: String,
    pub estimate: f64,
    pub clusters: u64,
    pub units: u64,
    /// exp(`estimate`) - 1, for an outcome paired as a log ratio.
    pub percent_change: Option<f64>,
    /// The least e-value of the claim's bounds in this group; `None` where no bound is held.
    pub e_value: Option<f64>,
    /// The group's intervals, never registered. The card holds them here only.
    pub intervals: Vec<Interval>,
    pub note: Option<String>,
    /// The outcome is registered.
    pub registered: bool,
}

pub fn results(after: &[Record], spec: &Spec, intervals: &[IntervalResult]) -> Vec<ResultRow> {
    let verdicts = first_verdicts(after);
    (estimates(after).into_iter())
        .map(|e| {
            let h = (spec.question.hypotheses.iter()).find(|h| h.id == e.hypothesis);
            let outcome = spec.outcomes.iter().find(|o| o.id == e.outcome);
            let log_ratio = outcome.is_some_and(|o| o.paired == Paired::LogRatio);
            let quantile = h.is_some_and(|h| {
                h.claim.is_some() && matches!(h.statistic, Some(StatisticSpec::Quantile(_)))
            });
            let e_value = (verdicts.iter().filter(|v| v.hypothesis == e.hypothesis))
                .flat_map(|v| &v.bounds)
                .filter(|b| b.stratum == e.stratum)
                .filter_map(|b| b.value)
                .reduce(f64::min);
            let intervals = (intervals.iter())
                .filter(|i| i.hypothesis == e.hypothesis && i.stratum == e.stratum)
                .map(Interval::from)
                .collect();
            ResultRow {
                percent_change: log_ratio.then(|| e.mean.exp() - 1.0),
                e_value,
                intervals,
                note: quantile.then(|| NO_QUANTILE_INTERVAL.into()),
                registered: outcome.is_some_and(|o| o.registered),
                hypothesis: e.hypothesis,
                stratum: e.stratum,
                outcome: e.outcome,
                estimate: e.mean,
                clusters: e.clusters,
                units: e.units,
            }
        })
        .collect()
}

/// The power gate's prediction for one hypothesis, as the register record holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sensitivity {
    pub hypothesis: String,
    pub power: f64,
    pub target: f64,
    pub power_at: f64,
    pub curve: Vec<Curve>,
    pub placed: Vec<Placed>,
}

/// Where one group's estimate falls on the curve toward `margin`: the nearest point at or
/// below it and at or above it; `None` past an end of the curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    pub margin: f64,
    pub stratum: String,
    pub estimate: f64,
    pub lower: Option<CurvePoint>,
    pub upper: Option<CurvePoint>,
}

fn place(curve: &Curve, stratum: &str, estimate: f64) -> Placed {
    let points = curve.points.iter();
    let by_mean = |a: &&CurvePoint, b: &&CurvePoint| a.mean.total_cmp(&b.mean);
    Placed {
        margin: curve.margin,
        stratum: stratum.into(),
        estimate,
        lower: (points
            .clone()
            .filter(|p| p.mean <= estimate)
            .max_by(by_mean))
        .cloned(),
        upper: (points.filter(|p| p.mean >= estimate).min_by(by_mean)).cloned(),
    }
}

/// The gate's curves from the register record `own`, with the estimates in `after`.
pub fn sensitivity(own: &Register, after: &[Record], spec: &Spec) -> Vec<Sensitivity> {
    let estimates = estimates(after);
    (own.gate.iter())
        .map(|g| {
            let strata = (spec.question.hypotheses.iter())
                .find(|h| h.id == g.id)
                .map(claim_strata)
                .unwrap_or_default();
            let held = (estimates.iter())
                .filter(|e| e.hypothesis == g.id && strata.contains(&e.stratum.as_str()));
            let placed = (g.curve.iter())
                .flat_map(|c| held.clone().map(|e| place(c, &e.stratum, e.mean)))
                .collect();
            Sensitivity {
                hypothesis: g.id.clone(),
                power: g.power,
                target: g.target,
                power_at: g.power_at,
                curve: g.curve.clone(),
                placed,
            }
        })
        .collect()
}

/// The machine each row ran in (disclosure part 4). A value the row records do not hold
/// links the run log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stack {
    /// Each value the rows name, sorted.
    pub isolation: Field<Vec<u8>>,
    pub launcher: Field<Vec<String>>,
    pub agent_sha256: Field<Vec<String>>,
    /// The box's machine config holds it; no record does.
    pub vcpus: Field<u32>,
    /// The plan's `row_memory_mib`.
    pub memory_cap_mib: Field<u32>,
}

/// The distinct values of `key` over the row records of `log`; `None` when no row holds one.
fn from_rows<T: Ord + DeserializeOwned>(log: &Log, key: &str) -> Option<Vec<T>> {
    let values: BTreeSet<T> = (rows(log).filter_map(|r| r.body.get(key)))
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect();
    (!values.is_empty()).then(|| values.into_iter().collect())
}

pub fn stack(log: &Log, spec: &Spec, events: Option<&Outside>) -> Stack {
    let levels: Option<Vec<u8>> = from_rows(log, "isolation");
    let agentless = (levels.as_ref()).is_some_and(|l| l.iter().all(|l| *l < AGENT_LEVEL));
    let agent_sha256 = match from_rows(log, "agent_sha256") {
        None if agentless => Field::NotApplicable,
        found => Field::of(found, events),
    };
    Stack {
        isolation: Field::of(levels, events),
        launcher: Field::of(from_rows(log, "launcher"), events),
        agent_sha256,
        vcpus: Field::of(None, None),
        memory_cap_mib: Field::of(spec.design.row_memory_mib, None),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Setup {
    /// Each input by path, with the digest of its stored copy; `None` for one that was absent.
    pub data: Vec<(String, Option<String>)>,
    /// Data grades are not defined yet (audit C6).
    pub data_grade: Field<String>,
    pub instrument: Instrument,
    /// Every value the box filled in for the plan.
    pub defaults: Value,
    /// Every value taken from another study.
    pub resolved: Vec<Resolved>,
    pub seeds: Seeds,
    pub stack: Stack,
}

/// The study's seed and how its rows' seeds derive from it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Seeds {
    /// The plan hash of the register record: every draw derives from it.
    pub root: Field<String>,
    pub rule: String,
    /// Row records that hold a seed, and those whose seed `rule` gives from `root`.
    pub rows: u64,
    pub derived: u64,
}

fn seeds(log: &Log, own: &Register) -> Seeds {
    let bytes = crate::spec::hash_bytes(&own.spec_hash);
    let seeded: Vec<&Record> = rows(log).filter(|r| r.body.get("seed").is_some()).collect();
    let derived = (seeded.iter())
        .filter(|r| {
            let index = r.body["recheck_of"].as_u64().or(r.row);
            let want = bytes.zip(index).map(|(h, i)| crate::seed::row_hex(&h, i));
            want.is_some_and(|w| r.body["seed"] == w.as_str())
        })
        .count();
    Seeds {
        root: Field::of(Some(own.spec_hash.clone()), None),
        rule: ROW_SEED.into(),
        rows: seeded.len() as u64,
        derived: derived as u64,
    }
}

pub fn setup(log: &Log, own: &Register, spec: &Spec, events: Option<&Outside>) -> Setup {
    Setup {
        data: (log.records.iter().find_map(Inputs::of)).map_or_else(Vec::new, |i| i.inputs),
        data_grade: Field::of(None, None),
        instrument: (log.records.iter().find_map(Analysis::of))
            .map(|a| a.instrument)
            .unwrap_or_default(),
        defaults: own.defaults.clone(),
        seeds: seeds(log, own),
        resolved: own.resolved.clone(),
        stack: stack(log, spec, events),
    }
}

/// A row record whose exit is not `ok`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailedRow {
    pub row: Option<u64>,
    pub stage: String,
    pub exit: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Surprises {
    pub failed: Vec<FailedRow>,
    /// Attempts past the first, over every row.
    pub retried: u64,
    pub clipped: Vec<GroupClip>,
    pub triggers: Vec<Trigger>,
    /// Each box version an analysis record names; more than one is drift.
    pub box_versions: Vec<String>,
}

fn text(r: &Record, key: &str) -> Option<String> {
    r.body.get(key).and_then(Value::as_str).map(str::to_string)
}

pub fn surprises(log: &Log) -> Surprises {
    let analyses: Vec<Analysis> = log.records.iter().filter_map(Analysis::of).collect();
    Surprises {
        failed: (rows(log).filter(|r| r.body["exit"] != "ok"))
            .map(|r| FailedRow {
                row: r.row,
                stage: text(r, "stage").unwrap_or_default(),
                exit: text(r, "exit").unwrap_or_default(),
                reason: text(r, "reason"),
            })
            .collect(),
        retried: Tally::of(&log.records).retried,
        clipped: (analyses.first()).map_or_else(Vec::new, |a| a.clipped.clone()),
        triggers: log.records.iter().filter_map(Trigger::of).collect(),
        box_versions: (analyses.into_iter().map(|a| a.box_version))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    }
}

/// Spend in the `row` and `spend` records, the box's CPU included, against the plan's budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub cpu_seconds: f64,
    pub wall_seconds: f64,
    /// Completed rows.
    pub rows: u64,
    /// Attempts whose CPU count the backend could not read; each was charged the row estimate.
    pub unknown_cpu: u64,
    /// The machine's rate is in its config; no record holds it.
    pub usd: Field<f64>,
    pub budget: Budget,
    pub telemetry: Totals,
}

pub fn cost(log: &Log, spec: &Spec) -> Cost {
    let t = Tally::of(&log.records);
    Cost {
        cpu_seconds: t.cpu,
        wall_seconds: t.wall,
        rows: t.rows,
        unknown_cpu: t.unknown_cpu,
        usd: Field::of(None, None),
        budget: spec.scope.budget.clone(),
        telemetry: Totals::of(rows(log).map(|r| &r.body)),
    }
}

/// The `ok` row records of one stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunGroup {
    pub stage: String,
    pub rows: u64,
}

/// An invalid attempt: a `row` record whose exit is not `ok`, or a `spend` record, which holds
/// an attempt no row record holds. `cpu_seconds` is the attempt's own; the box's is left out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    pub row: Option<u64>,
    pub kind: String,
    pub exit: Option<String>,
    pub reason: Option<String>,
    pub cpu_seconds: Option<f64>,
}

/// Every execution the log records, invalid ones included (`UNRECORDED` for the rest).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Runs {
    pub ok: Vec<RunGroup>,
    /// In log order.
    pub attempts: Vec<Attempt>,
    /// The run sessions are events, which no key signs.
    pub sessions: Field<u64>,
    /// The reason in the study's `stop` record.
    pub stopped: Option<String>,
}

pub fn runs(log: &Log, events: Option<&Outside>) -> Runs {
    let mut ok: BTreeMap<String, u64> = BTreeMap::new();
    let mut attempts = vec![];
    for r in &log.records {
        let row = r.kind == "row";
        if row && r.body["exit"] == "ok" {
            *ok.entry(text(r, "stage").unwrap_or_default()).or_default() += 1;
        } else if row || r.kind == "spend" {
            attempts.push(Attempt {
                row: r.row,
                kind: r.kind.clone(),
                exit: text(r, "exit"),
                reason: text(r, "reason"),
                cpu_seconds: r.body["cpu_seconds"].as_f64(),
            });
        }
    }
    Runs {
        ok: (ok.into_iter())
            .map(|(stage, rows)| RunGroup { stage, rows })
            .collect(),
        attempts,
        sessions: Field::of(None, events),
        stopped: crate::cli::stopped_why(&log.records),
    }
}
