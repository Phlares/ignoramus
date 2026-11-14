//! `report`: the verdicts of a finished study, with the clip counts of its mean claims.
//! Appends one `opened` record, so a reader of the log sees that the results were read.
use super::{CliError, Ctx, Done, Run, located, not_registered, out};
use crate::card::{self, Card, Opened, render};
use crate::derived::OPENED;
use crate::diag::printable;
use crate::records::{self, RecordDraft, RecordError};
use serde_json::json;
use std::path::Path;

/// The way `report` writes the card.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Format {
    Text,
    Json,
    Md,
    Tex,
}

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

pub fn report(ctx: &Ctx, id: &str) -> Result<Card, CliError> {
    let (home, log) = located(ctx, id)?;
    let key = crate::keys::load(home)
        .map_err(|e| CliError::usage(format!("box key: {e}")))?
        .ok_or_else(|| not_registered(id))?;
    let mut built = None;
    let appended = records::append_with(&log, &key, |all| -> Result<_, Stop> {
        built = Some(card::build(home, id, all)?);
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
    let mut card = built.ok_or_else(|| CliError::usage("no card built"))?;
    card.opened = Opened {
        seq: opened.seq,
        ts: opened.ts.clone(),
    };
    Ok(card)
}

fn text(r: &Card) -> String {
    let mut s = format!(
        "study {}: finished {}, {} records verified
",
        r.study, r.finished_at, r.records
    );
    if let Some(f) = r.family.as_object() {
        let members = f["members"].as_array().into_iter().flatten();
        let waiting: Vec<String> = (members.clone())
            .filter(|m| m["state"] != "finished")
            .map(|m| {
                let field = |k: &str| m[k].as_str().unwrap_or_default();
                format!(" {} {}", field("study"), field("state"))
            })
            .collect();
        let root = f["root"].as_str().unwrap_or_default();
        s.push_str(&format!("family {root}:{}\n", waiting.join(",")));
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
    for h in &r.hypotheses {
        s.push_str(&format!("{}: {} ({})", h.hypothesis, h.verdict, h.basis));
        if let Some(e) = h.e_value {
            s.push_str(&format!(", e-value {e}"));
        }
        if let Some(l) = h.level {
            s.push_str(&format!(", level {l}"));
        }
        if let Some(why) = &h.reason {
            s.push_str(&format!(": {why}"));
        }
        s.push('\n');
        for c in h.clipped.iter().flatten() {
            let stratum = if c.stratum.is_empty() {
                "all"
            } else {
                &c.stratum
            };
            s.push_str(&format!(
                "  clipped in {stratum}: {} below lo, {} above hi, of {} units\n",
                c.below, c.above, c.units
            ));
        }
    }
    for c in &r.children {
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

/// Writes the LaTeX fragments of `card` into `dir`; the count of files.
fn write_tex(card: &Card, dir: &Path) -> Result<usize, CliError> {
    let io = |e: std::io::Error| CliError::usage(format!("{}: {e}", dir.display()));
    std::fs::create_dir_all(dir).map_err(io)?;
    let files = render::latex(card).map_err(|e| CliError::usage(format!("report: {e}")))?;
    for (name, text) in &files {
        std::fs::write(dir.join(name), text).map_err(io)?;
    }
    Ok(files.len())
}

pub fn run(ctx: &Ctx, id: &str, format: Format, dir: Option<&Path>) -> Run {
    // Refused before `report` appends its record.
    match (format, dir) {
        (Format::Tex, None) => {
            return Err(CliError::usage("report: --format tex needs --out <dir>"));
        }
        (Format::Tex, Some(_)) | (_, None) => {}
        (_, Some(_)) => {
            return Err(CliError::usage(
                "report: --out applies to --format tex only",
            ));
        }
    }
    let r = report(ctx, id)?;
    let shown = match (format, dir) {
        (Format::Json, _) => {
            serde_json::to_string_pretty(&r).map_err(|e| CliError::usage(e.to_string()))? + "\n"
        }
        (Format::Md, _) => render::markdown(&r),
        (Format::Tex, Some(dir)) => {
            format!("wrote {} files to {}\n", write_tex(&r, dir)?, dir.display())
        }
        _ => text(&r),
    };
    out(&shown)?;
    Ok(Done::exit(0))
}
