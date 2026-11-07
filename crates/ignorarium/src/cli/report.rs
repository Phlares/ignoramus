//! `report`: the verdicts of a finished study, with the clip counts of its mean claims.
//! Appends one `opened` record, so a reader of the log sees that the results were read.
use super::check::HomeRecords;
use super::{CliError, Ctx, Done, Run, located, not_registered, out};
use crate::derived::{Analysis, ESTIMATE, GroupClip, OPENED, Register, StudyState, Verdict};
use crate::records::{self, Record, RecordDraft, RecordError};
use crate::rules::BoxRecords;
use crate::stream::{self, Log, LogName};
use serde_json::{Value, json};

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

/// The time the results were complete: the close of the study's stream slot when it holds
/// one, else its last estimate or verdict record.
fn finished_at(
    ctx: &Ctx,
    key: &ed25519_dalek::VerifyingKey,
    id: &str,
    register: &Register,
    analysis: &Analysis,
    after: &[Record],
) -> Result<String, CliError> {
    let io = |e: &dyn std::fmt::Display| CliError::usage(format!("cannot read the stream: {e}"));
    if let (Some(slot), Some(alpha)) = (analysis.slot, &register.alpha) {
        let name = LogName::parse(&alpha.log).map_err(|e| io(&e))?;
        let path = Log::of(ctx.home()?, name).path().to_path_buf();
        let all = records::read_all(&path, key).map_err(|e| io(&e))?;
        let close = all
            .iter()
            .find(|r| r.kind == stream::CLOSE && r.body["study"] == id && r.body["slot"] == slot);
        if let Some(r) = close {
            return Ok(r.ts.clone());
        }
    }
    let last = after
        .iter()
        .rfind(|r| r.kind == ESTIMATE || r.kind == crate::derived::VERDICT);
    Ok(last.map(|r| r.ts.clone()).unwrap_or_default())
}

fn clips(analysis: &Analysis, h: &str) -> Value {
    let mine: Vec<&GroupClip> = analysis
        .clipped
        .iter()
        .filter(|c| c.hypothesis == h)
        .collect();
    if mine.is_empty() {
        return Value::Null;
    }
    mine.iter()
        .map(|c| json!({ "stratum": c.stratum, "below": c.below, "above": c.above, "units": c.units }))
        .collect()
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
        let box_records = HomeRecords {
            home: home.clone(),
            key: Ok(Some(verifying)),
        };
        let state = StudyState::of(Some(all.to_vec()), |log, slot| {
            box_records.stream_closed(log, id, slot)
        })
        .map_err(|e| CliError::usage(format!("cannot report {id}: {e}")))?;
        let StudyState::Finished(after) = state else {
            return Err(CliError::refuse(format!("cannot report {id}: not finished")).into());
        };
        let analysis = (all.iter().find_map(Analysis::of)).ok_or_else(|| not_registered(id))?;
        let hypotheses: Vec<Value> = after
            .iter()
            .filter_map(Verdict::of)
            .map(|v| {
                json!({
                    "hypothesis": v.hypothesis, "verdict": v.verdict, "basis": v.basis,
                    "e_value": v.e_value, "level": v.level, "reason": v.reason,
                    "clipped": clips(&analysis, &v.hypothesis),
                })
            })
            .collect();
        value = json!({
            "study": id,
            "finished_at": finished_at(ctx, &verifying, id, &register, &analysis, &after)?,
            "records": all.len(),
            "hypotheses": hypotheses,
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

fn num(v: &Value, places: usize) -> Option<String> {
    v.as_f64().map(|x| format!("{x:.places$}"))
}

fn text(r: &Value) -> String {
    let id = r["study"].as_str().unwrap_or_default();
    let mut s = format!(
        "study {id}: finished {}, {} records verified
",
        r["finished_at"].as_str().unwrap_or_default(),
        r["records"]
    );
    for h in r["hypotheses"].as_array().map_or(&[][..], Vec::as_slice) {
        s.push_str(&format!(
            "{}: {} ({})",
            h["hypothesis"].as_str().unwrap_or_default(),
            h["verdict"].as_str().unwrap_or_default(),
            h["basis"].as_str().unwrap_or_default()
        ));
        if let Some(e) = num(&h["e_value"], 2) {
            s.push_str(&format!(", e-value {e}"));
        }
        if let Some(l) = num(&h["level"], 4) {
            s.push_str(&format!(", level {l}"));
        }
        if let Some(why) = h["reason"].as_str() {
            s.push_str(&format!(": {why}"));
        }
        s.push('\n');
        for c in h["clipped"].as_array().map_or(&[][..], Vec::as_slice) {
            let stratum = match c["stratum"].as_str() {
                Some("") | None => "all",
                Some(x) => x,
            };
            s.push_str(&format!(
                "  clipped in {stratum}: {} below lo, {} above hi, of {} units
",
                c["below"], c["above"], c["units"]
            ));
        }
    }
    s
}

pub fn run(ctx: &Ctx, id: &str, json: bool) -> Run {
    let r = report(ctx, id)?;
    let shown = if json {
        serde_json::to_string_pretty(&r).map_err(|e| CliError::usage(e.to_string()))?
            + "
"
    } else {
        text(&r)
    };
    out(&shown)?;
    Ok(Done::exit(0))
}
