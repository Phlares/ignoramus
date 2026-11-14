//! The study card: everything `report` says about a finished study, as one structure.
//! Each output format renders this structure; none reads the log on its own.
use crate::cli::check::HomeRecords;
use crate::cli::status::{relabelled, shown_labels};
use crate::cli::{CliError, not_registered, stopped_why};
use crate::derived::{
    Analysis, ChildEstimate, Estimate, OPENED, Register, SpawnStop, StudyState, Verdict,
};
use crate::home::Home;
use crate::records::{Log, Record};
use crate::rules::BoxRecords;
use crate::runner::stored_spec;
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
}

/// How many units of one stratum a mean claim clipped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub stratum: String,
    pub below: u64,
    pub above: u64,
    pub units: u64,
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
    let hypotheses: Vec<HypothesisCard> = after
        .iter()
        .filter_map(Verdict::of)
        .filter(|v| seen.insert(v.hypothesis.clone()))
        .map(|v| HypothesisCard {
            clipped: clips(&analysis, &v.hypothesis),
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
        let (spec, _, hash, _) = stored_spec(&home.study_dir(id))
            .map_err(|e| CliError::usage(format!("cannot report {id}: {e}")))?;
        shown_labels(&spec, &hash)
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
    })
}
