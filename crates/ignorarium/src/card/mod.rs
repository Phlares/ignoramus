//! The study card: everything `report` says about a finished study, as one structure.
//! Each output format renders this structure; none reads the log on its own.
use crate::analysis::{IntervalResult, Source};
use crate::cli::check::HomeRecords;
use crate::cli::status::{relabelled, shown_labels};
use crate::cli::{CliError, not_registered, stopped_why};
use crate::derived::{
    Analysis, ChildEstimate, Estimate, OPENED, PinDeviated, Register, SpawnStop, StudyState,
    Verdict,
};
use crate::deviations::{Deviations, Timing};
use crate::home::Home;
use crate::records::{Log, Record};
use crate::rules::BoxRecords;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// The version of the card's JSON form.
pub const CARD_VERSION: u32 = 1;

/// The `opened` record that marks the card as read. `build` leaves it zeroed; the caller that
/// appends the record fills it in.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Opened {
    pub seq: u64,
    pub ts: String,
}

/// One hypothesis and its verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HypothesisCard {
    pub hypothesis: String,
    pub verdict: String,
    pub basis: String,
    pub e_value: Option<f64>,
    pub level: Option<f64>,
    pub reason: Option<String>,
    pub clipped: Option<Vec<Clip>>,
    pub intervals: Vec<Interval>,
}

/// The method every `Interval` names.
pub const INTERVAL_METHOD: &str = "inverted registered e-process";

/// One bound of a claim in one stratum, its registered test inverted over a grid of margins
/// (`analysis::intervals`). The method was chosen after the results were known, so it is
/// never registered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Interval {
    pub stratum: String,
    pub side: String,
    pub estimate: f64,
    /// The tightest margin the test shows; `None` when no grid margin passes, or when the
    /// margins that pass are no interval (`contiguous` false).
    pub bound: Option<f64>,
    pub method: String,
    pub grid: IntervalGrid,
    pub contiguous: bool,
    pub registered: bool,
}

/// The margins tried: from `from` toward `to` in steps of `step`, both ends left out.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IntervalGrid {
    pub from: f64,
    pub to: f64,
    pub step: f64,
}

impl From<&IntervalResult> for Interval {
    fn from(r: &IntervalResult) -> Interval {
        let i = &r.inversion;
        Interval {
            stratum: r.stratum.clone(),
            side: r.side.clone(),
            estimate: r.estimate,
            bound: i.margin.filter(|_| i.contiguous),
            method: INTERVAL_METHOD.into(),
            grid: IntervalGrid {
                from: i.grid.from,
                to: i.grid.to,
                step: i.grid.step,
            },
            contiguous: i.contiguous,
            registered: false,
        }
    }
}

/// How many units of one stratum a mean claim clipped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub stratum: String,
    pub below: u64,
    pub above: u64,
    pub units: u64,
}

/// One row of the deviation table. A signed row comes from the register record; an unsigned
/// row comes from `deviations.toml`, which no key signs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mark", rename_all = "lowercase")]
pub enum DeviationRow {
    Signed {
        from: String,
        reason: String,
        timing: String,
        differs: Vec<String>,
    },
    Unsigned {
        /// `deviation` or `unregistered`.
        entry: String,
        /// The deviation's type, or the unregistered step.
        subject: String,
        reason: String,
        timing: Timing,
        original: Option<String>,
        impact: Option<String>,
        source: String,
        /// SHA-256 of `deviations.toml`.
        sha256: String,
    },
}

/// The deviation table: the signed rows first, then the file's rows.
pub fn deviation_table(signed: &[PinDeviated], file: Option<&Deviations>) -> Vec<DeviationRow> {
    let signed = signed.iter().map(|d| DeviationRow::Signed {
        from: d.from.clone(),
        reason: d.reason.clone(),
        timing: d.timing.clone(),
        differs: d.differs.clone(),
    });
    let unsigned = file.into_iter().flat_map(|f| {
        let hash = &f.sha256;
        let deviations = f.deviation.iter().map(move |d| DeviationRow::Unsigned {
            entry: "deviation".into(),
            subject: d.kind.clone(),
            reason: d.reason.clone(),
            timing: d.timing,
            original: Some(d.original.clone()),
            impact: Some(d.impact.clone()),
            source: d.source.clone(),
            sha256: hash.clone(),
        });
        let unregistered = f.unregistered.iter().map(move |u| DeviationRow::Unsigned {
            entry: "unregistered".into(),
            subject: u.step.clone(),
            reason: u.reason.clone(),
            timing: u.timing,
            original: None,
            impact: None,
            source: u.source.clone(),
            sha256: hash.clone(),
        });
        deviations.chain(unregistered)
    });
    signed.chain(unsigned).collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub card_version: u32,
    pub study: String,
    pub finished_at: String,
    pub records: u64,
    pub opened: Opened,
    pub hypotheses: Vec<HypothesisCard>,
    pub children: Vec<Value>,
    pub family: Value,
    pub deviations: Vec<DeviationRow>,
}

/// The time the results were complete: the later of the last counted estimate or verdict
/// record and, when the study holds a stream slot, the close of that slot. A record the report
/// drops as a repeat does not count. With neither, the analysis record's time.
fn finished_at(close: Option<String>, analysed: &str, after: &[Record]) -> String {
    let mut estimates = BTreeSet::new();
    let mut verdicts = BTreeSet::new();
    let mut stamps: Vec<String> = (after.iter())
        .filter(|r| match (Estimate::of(r), Verdict::of(r)) {
            (Some(e), _) => estimates.insert(e.identity()),
            (_, Some(v)) => verdicts.insert(v.hypothesis),
            _ => false,
        })
        .map(|r| r.ts.clone())
        .collect();
    stamps.extend(close);
    stamps
        .into_iter()
        .max()
        .unwrap_or_else(|| analysed.to_string())
}

/// How a finished study finished: its records after the analysis, and when.
struct Finish<'a> {
    analysed: &'a Record,
    after: Vec<Record>,
    at: String,
}

/// The finish of study `id`, whose records are `all`; `None` when it is not finished.
fn standing<'a>(
    records: &HomeRecords,
    id: &str,
    all: &'a [Record],
) -> Result<Option<Finish<'a>>, String> {
    let mut close = None;
    let state = StudyState::of(Some(all.to_vec()), |log, slot| {
        close = records.stream_closed_at(log, id, slot)?;
        Ok(close.is_some())
    })?;
    let StudyState::Finished(after) = state else {
        return Ok(None);
    };
    let analysed = (all.iter().find(|r| Analysis::of(r).is_some()))
        .ok_or_else(|| format!("study {id} has no analysis record"))?;
    let at = finished_at(close, &analysed.ts, &after);
    Ok(Some(Finish {
        analysed,
        after,
        at,
    }))
}

/// The family of study `id`, whose register record holds `own`: its root and every other
/// member, root first, each with its state. `at` is when `id` finished.
fn family_block(
    records: &HomeRecords,
    id: &str,
    own: &Register,
    at: &str,
) -> Result<Value, String> {
    let (root, lock) = match &own.family {
        Some(lock) => (id.to_string(), lock.clone()),
        None => match records.family_of(id)? {
            Some(found) => (found.root, found.lock),
            None => return Ok(Value::Null),
        },
    };
    let others = std::iter::once(root.clone())
        .chain(lock.members.into_iter().map(|m| m.id))
        .filter(|m| m != id);
    let mut members = vec![];
    for other in others {
        let fault = |e: String| format!("study {other}: {e}");
        let mut m = json!({
            "study": other, "state": "not registered",
            "finished_at": null, "first_opened": null, "before": null,
        });
        if let Some(all) = records.study(&other).map_err(fault)? {
            m["state"] = json!("not finished");
            if let Some(f) = standing(records, &other, &all).map_err(fault)? {
                m["state"] = json!("finished");
                m["first_opened"] =
                    json!((all.iter().find(|r| r.kind == OPENED)).map(|r| r.ts.clone()));
                m["before"] = json!(f.at.as_str() < at);
                m["finished_at"] = json!(f.at);
            }
        }
        members.push(m);
    }
    Ok(json!({ "root": root, "members": members }))
}

fn clips(analysis: &Analysis, h: &str) -> Option<Vec<Clip>> {
    let mine: Vec<Clip> = (analysis.clipped.iter())
        .filter(|c| c.hypothesis == h)
        .map(|c| Clip {
            stratum: c.stratum.clone(),
            below: c.below,
            above: c.above,
            units: c.units,
        })
        .collect();
    (!mine.is_empty()).then_some(mine)
}

/// The card of finished study `id`, whose log is `log`. Refuses a study that is not finished.
pub fn build(home: &Home, id: &str, log: &Log) -> Result<Card, CliError> {
    let key = crate::keys::load(home)
        .map_err(|e| CliError::usage(format!("box key: {e}")))?
        .ok_or_else(|| not_registered(id))?;
    let all = &log.records;
    if let Some(r) = all.iter().find(|r| r.study != id) {
        return Err(CliError::refuse(format!(
            "cannot report {id}: record {} belongs to study {}",
            r.seq, r.study
        )));
    }
    let own = (all.iter().find_map(Register::of)).ok_or_else(|| not_registered(id))?;
    let box_records = HomeRecords::signed(home, &key);
    let refuse = |e: String| CliError::refuse(format!("cannot report {id}: {e}"));
    let Some(Finish {
        analysed,
        after,
        at,
    }) = standing(&box_records, id, all).map_err(refuse)?
    else {
        let said = match stopped_why(all) {
            Some(why) => format!("study {id} is stopped: {why}; it has no report"),
            None => format!("study {id} is not finished; status shows its progress"),
        };
        return Err(CliError::refuse(said));
    };
    let analysis = Analysis::of(analysed).ok_or_else(|| not_registered(id))?;
    let mut seen = BTreeSet::new();
    let verdicts: Vec<Verdict> = (after.iter().filter_map(Verdict::of))
        .filter(|v| seen.insert(v.hypothesis.clone()))
        .collect();
    let file = crate::deviations::load(&home.study_dir(id)).map_err(CliError::usage)?;
    let text = std::fs::read(home.study_dir(id).join("spec.toml"))
        .map_err(|e| CliError::usage(format!("cannot report {id}: its plan: {e}")))?;
    let source = Source::read(&text, all, log.data_hashes.as_ref()).map_err(refuse)?;
    let intervals = source.intervals(&verdicts);
    let hypotheses: Vec<HypothesisCard> = (verdicts.into_iter())
        .map(|v| HypothesisCard {
            clipped: clips(&analysis, &v.hypothesis),
            intervals: (intervals.iter())
                .filter(|r| r.hypothesis == v.hypothesis)
                .map(Interval::from)
                .collect(),
            hypothesis: v.hypothesis,
            verdict: v.verdict,
            basis: v.basis,
            e_value: v.e_value,
            level: v.level,
            reason: v.reason,
        })
        .collect();
    let mut seen = BTreeSet::new();
    let kids: Vec<ChildEstimate> = (all.iter().filter_map(ChildEstimate::of))
        .filter(|e| seen.insert((e.child.clone(), e.outcome.clone())))
        .collect();
    let shown = if kids.is_empty() {
        vec![]
    } else {
        shown_labels(&source.spec, &source.hash)
    };
    let stopped: BTreeSet<String> = (all.iter().filter_map(SpawnStop::of))
        .flat_map(|s| s.children)
        .collect();
    let children: Vec<Value> = (kids.iter())
        .map(|e| {
            json!({
                "child": relabelled(&shown, &e.child),
                "parent": relabelled(&shown, &e.parent),
                "operator": e.operator, "value": e.value, "outcome": e.outcome,
                "clusters": e.clusters, "units": e.units, "mean": e.mean, "repeats": e.repeats,
                "stopped": stopped.contains(&e.child),
            })
        })
        .collect();
    let family = family_block(&box_records, id, &own, &at).map_err(refuse)?;
    Ok(Card {
        card_version: CARD_VERSION,
        study: id.to_string(),
        finished_at: at,
        records: all.len() as u64,
        opened: Opened::default(),
        hypotheses,
        children,
        family,
        deviations: deviation_table(&own.deviations, file.as_ref()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use evidence::claim::{Grid, Inversion};

    #[test]
    fn a_gap_in_the_passing_margins_prints_no_bound() {
        let result = |contiguous| IntervalResult {
            hypothesis: "H1".into(),
            stratum: String::new(),
            side: "below".into(),
            estimate: 0.0,
            inversion: Inversion {
                margin: Some(0.5),
                contiguous,
                grid: Grid {
                    from: 0.0,
                    to: 1.0,
                    step: 0.5,
                },
                threshold: 2.0,
            },
        };
        assert_eq!(Interval::from(&result(true)).bound, Some(0.5));
        let gap = Interval::from(&result(false));
        assert_eq!((gap.bound, gap.contiguous), (None, false));
    }
}
