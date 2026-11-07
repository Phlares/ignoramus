//! `report`: the verdicts of a finished study, with the clip counts of its mean claims.
//! Appends one `opened` record, so a reader of the log sees that the results were read.
use super::check::HomeRecords;
use super::status::{relabelled, shown_labels};
use super::{CliError, Ctx, Done, Run, located, not_registered, out, stopped_why};
use crate::derived::{
    Analysis, ChildEstimate, Estimate, OPENED, Register, SpawnStop, StudyState, Verdict,
};
use crate::diag::printable;
use crate::records::{self, Record, RecordDraft, RecordError};
use crate::runner::stored_spec;
use crate::rules::BoxRecords;
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
    let mut value = Value::Null;
    let appended = records::append_with(&log, &key, |all| -> Result<_, Stop> {
        if let Some(r) = all.iter().find(|r| r.study != id) {
            return Err(CliError::refuse(format!(
                "cannot report {id}: record {} belongs to study {}",
                r.seq, r.study
            ))
            .into());
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
            return Err(CliError::refuse(said).into());
        };
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
        let family = family_block(&box_records, id, &own, &at).map_err(refuse)?;
        value = json!({
            "study": id,
            "finished_at": at,
            "records": all.len(),
            "hypotheses": hypotheses,
            "children": children,
            "family": family,
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
    if let Some(f) = r["family"].as_object() {
        let members = f["members"].as_array().into_iter().flatten();
        let waiting: Vec<String> = (members.clone())
            .filter(|m| m["state"] != "finished")
            .map(|m| {
                let field = |k: &str| m[k].as_str().unwrap_or_default();
                format!(" {} {}", field("study"), field("state"))
            })
            .collect();
        let root = f["root"].as_str().unwrap_or_default();
        s.push_str(&format!(
            "family {root}:{}
",
            waiting.join(",")
        ));
        for m in members.filter(|m| m["state"] == "finished") {
            s.push_str(&format!(
                "  {} finished {}, first opened {}, {} this study finished\n",
                m["study"].as_str().unwrap_or_default(),
                m["finished_at"].as_str().unwrap_or_default(),
                m["first_opened"].as_str().unwrap_or("never"),
                if m["before"] == true {
                    "before"
                } else {
                    "after"
                }
            ));
        }
    }
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
