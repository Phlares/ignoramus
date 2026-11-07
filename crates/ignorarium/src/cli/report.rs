//! `report`: the verdicts of a finished study, with the clip counts of its mean claims.
//! Appends one `opened` record, so a reader of the log sees that the results were read.
use super::check::HomeRecords;
use super::status::{relabelled, shown_labels};
use super::{CliError, Ctx, Done, Run, located, not_registered, out, stopped_why};
use crate::derived::{
    Analysis, ChildEstimate, Estimate, OPENED, Register, SpawnStop, StudyState, Verdict,
};
use crate::diag::printable;
use crate::home::Home;
use crate::records::{self, Record, RecordDraft, RecordError};
use crate::rules::BoxRecords;
use crate::runner::stored_spec;
use crate::stream::{self, Log, LogName};
use ed25519_dalek::VerifyingKey;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// What stops the append: a refusal of this verb, or the log's own fault.
enum Stop {
    Verb(CliError),
    Log(RecordError),
}

impl From<RecordError> for Stop {
    fn from(e: RecordError) -> Self {
        Stop::Log(e)
    }
}

impl From<CliError> for Stop {
    fn from(e: CliError) -> Self {
        Stop::Verb(e)
    }
}

/// The time the results were complete: the later of the last counted estimate or verdict
/// record and, when the study holds a stream slot, the close of that slot. A record the report
/// drops as a repeat does not count. With neither, the analysis record's time.
fn finished_at(
    home: &Home,
    key: &VerifyingKey,
    id: &str,
    slot: Option<(&str, u64)>,
    analysed: &str,
    after: &[Record],
) -> Result<String, CliError> {
    let broken = |e: &dyn std::fmt::Display| CliError::refuse(format!("cannot report {id}: {e}"));
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
    if let Some((log, slot)) = slot {
        let name = LogName::parse(log).map_err(|e| broken(&e))?;
        let all = records::read_all(Log::of(home, name).path(), key).map_err(|e| broken(&e))?;
        stamps.extend(
            (all.iter())
                .filter(|r| {
                    r.kind == stream::CLOSE && r.body["study"] == id && r.body["slot"] == slot
                })
                .map(|r| r.ts.clone()),
        );
    }
    Ok(stamps
        .into_iter()
        .max()
        .unwrap_or_else(|| analysed.to_string()))
}

fn clips(analysis: &Analysis, h: &str) -> Value {
    let mine: Vec<Value> = (analysis.clipped.iter())
        .filter(|c| c.hypothesis == h)
        .map(|c| json!({ "stratum": c.stratum, "below": c.below, "above": c.above, "units": c.units }))
        .collect();
    if mine.is_empty() {
        Value::Null
    } else {
        Value::Array(mine)
    }
}

pub fn report(ctx: &Ctx, id: &str) -> Result<Value, CliError> {
    let (home, log) = located(ctx, id)?;
    let key = crate::keys::load(home)
        .map_err(|e| CliError::usage(format!("box key: {e}")))?
        .ok_or_else(|| not_registered(id))?;
    let verifying = key.verifying_key();
    let mut value = Value::Null;
    let appended = records::append_with(&log, &key, |all| -> Result<_, Stop> {
        if let Some(r) = all.iter().find(|r| r.study != id) {
            return Err(CliError::refuse(format!(
                "cannot report {id}: record {} belongs to study {}",
                r.seq, r.study
            ))
            .into());
        }
        let register = (all.iter().find_map(Register::of)).ok_or_else(|| not_registered(id))?;
        let box_records = HomeRecords::signed(home, &key);
        let state = StudyState::of(Some(all.to_vec()), |log, slot| {
            box_records.stream_closed(log, id, slot)
        })
        .map_err(|e| CliError::refuse(format!("cannot report {id}: {e}")))?;
        let StudyState::Finished(after) = state else {
            let said = match stopped_why(all) {
                Some(why) => format!("study {id} is stopped: {why}; it has no report"),
                None => format!("study {id} is not finished; status shows its progress"),
            };
            return Err(CliError::refuse(said).into());
        };
        let analysed =
            (all.iter().find(|r| Analysis::of(r).is_some())).ok_or_else(|| not_registered(id))?;
        let analysis = Analysis::of(analysed).ok_or_else(|| not_registered(id))?;
        let mut seen = BTreeSet::new();
        let hypotheses: Vec<Value> = after
            .iter()
            .filter_map(Verdict::of)
            .filter(|v| seen.insert(v.hypothesis.clone()))
            .map(|v| {
                json!({
                    "hypothesis": v.hypothesis, "verdict": v.verdict, "basis": v.basis,
                    "e_value": v.e_value, "level": v.level, "reason": v.reason,
                    "clipped": clips(&analysis, &v.hypothesis),
                })
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
        value = json!({
            "study": id,
            "finished_at": finished_at(
                home,
                &verifying,
                id,
                analysis.slot.map(|n| (register.log.as_str(), n)),
                &analysed.ts,
                &after,
            )?,
            "records": all.len(),
            "hypotheses": hypotheses,
            "children": children,
        });
        Ok(vec![RecordDraft::new(id, OPENED, json!({}))])
    })
    .map_err(|e| match e {
        Stop::Verb(e) => e,
        Stop::Log(RecordError::Log(v)) => CliError::refuse(format!("cannot report {id}: {v}")),
        Stop::Log(e) => CliError::usage(format!("cannot report {id}: {e}")),
    })?;
    let opened = appended
        .first()
        .ok_or_else(|| CliError::usage("no record appended"))?;
    value["opened"] = json!({ "seq": opened.seq, "ts": opened.ts });
    Ok(value)
}

fn text(r: &Value) -> String {
    let id = r["study"].as_str().unwrap_or_default();
    let mut s = format!(
        "study {id}: finished {}, {} records verified\n",
        r["finished_at"].as_str().unwrap_or_default(),
        r["records"]
    );
    for h in r["hypotheses"].as_array().into_iter().flatten() {
        s.push_str(&format!(
            "{}: {} ({})",
            h["hypothesis"].as_str().unwrap_or_default(),
            h["verdict"].as_str().unwrap_or_default(),
            h["basis"].as_str().unwrap_or_default()
        ));
        if let Some(e) = h["e_value"].as_f64() {
            s.push_str(&format!(", e-value {e}"));
        }
        if let Some(l) = h["level"].as_f64() {
            s.push_str(&format!(", level {l}"));
        }
        if let Some(why) = h["reason"].as_str() {
            s.push_str(&format!(": {why}"));
        }
        s.push('\n');
        for c in h["clipped"].as_array().into_iter().flatten() {
            let stratum = match c["stratum"].as_str() {
                Some("") | None => "all",
                Some(x) => x,
            };
            s.push_str(&format!(
                "  clipped in {stratum}: {} below lo, {} above hi, of {} units\n",
                c["below"], c["above"], c["units"]
            ));
        }
    }
    for c in r["children"].as_array().into_iter().flatten() {
        let partial = if c["stopped"] == true {
            ", stopped (partial: only the rows that ran)"
        } else {
            ""
        };
        s.push_str(&format!(
            "child {}: {} mean {} over {} clusters, {} units, {} repeats{partial}\n",
            printable(c["child"].as_str().unwrap_or_default()),
            c["outcome"].as_str().unwrap_or_default(),
            c["mean"].as_f64().unwrap_or(f64::NAN),
            c["clusters"],
            c["units"],
            c["repeats"]
        ));
    }
    s
}

pub fn run(ctx: &Ctx, id: &str, json: bool) -> Run {
    let r = report(ctx, id)?;
    let shown = if json {
        serde_json::to_string_pretty(&r).map_err(|e| CliError::usage(e.to_string()))? + "\n"
    } else {
        text(&r)
    };
    out(&shown)?;
    Ok(Done::exit(0))
}
