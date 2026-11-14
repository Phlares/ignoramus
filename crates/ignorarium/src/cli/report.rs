//! `report`: the verdicts of a finished study, with the clip counts of its mean claims.
//! Appends one `opened` record, so a reader of the log sees that the results were read.
use super::{CliError, Ctx, Done, Run, located, not_registered, out};
use crate::canon::sha256_hex;
use crate::card::{self, Card, Opened, readings, recipe, render};
use crate::derived::OPENED;
use crate::diag::printable;
use crate::home::Home;
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
    report_with(ctx, id, |_| Ok(())).map(|(card, ())| card)
}

/// `report`, with `before` run on the built card ahead of the `opened` append. When `before`
/// refuses, nothing is appended.
fn report_with<T>(
    ctx: &Ctx,
    id: &str,
    before: impl FnOnce(&Card) -> Result<T, CliError>,
) -> Result<(Card, T), CliError> {
    let (home, log) = located(ctx, id)?;
    let key = crate::keys::load(home)
        .map_err(|e| CliError::usage(format!("box key: {e}")))?
        .ok_or_else(|| not_registered(id))?;
    let mut built = None;
    let appended = records::append_with(&log, &key, |all| -> Result<_, Stop> {
        let card = card::build(home, id, all)?;
        built = Some((before(&card)?, card));
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
    let (kept, mut card) = built.ok_or_else(|| CliError::usage("no card built"))?;
    card.opened = Opened {
        seq: opened.seq,
        ts: opened.ts.clone(),
    };
    Ok((card, kept))
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

fn write_files(dir: &Path, files: &[(String, String)]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    files
        .iter()
        .try_for_each(|(name, text)| std::fs::write(dir.join(name), text))
}

/// Writes the LaTeX fragments of `card` into `dir`; the count of files.
fn write_tex(card: &Card, dir: &Path) -> Result<usize, CliError> {
    let files = render::latex(card).map_err(|e| CliError::usage(format!("report: {e}")))?;
    write_files(dir, &files).map_err(|e| CliError::usage(format!("{}: {e}", dir.display())))?;
    Ok(files.len())
}

const CARD_FILES: [&str; 4] = ["card.json", "card.md", "manifest.sha256", readings::FILE];

/// What `--card` writes besides `card.json`, `card.md` and `readings.md`. Rendered and checked
/// before the `opened` append.
struct Parts {
    tex: Vec<(String, String)>,
    figures: Vec<(String, String)>,
    manifest: String,
}

/// Refuses an `--out` folder that holds a file this command does not write.
fn check_folder(dir: &Path, parts: &Parts) -> Result<(), CliError> {
    if dir.exists() && !dir.is_dir() {
        return Err(CliError::usage(format!("{}: not a folder", dir.display())));
    }
    let listed = |files: &[(String, String)], name: &str| files.iter().any(|(n, _)| n == name);
    for (top, at) in [(true, dir.to_path_buf()), (false, dir.join("tex"))] {
        let Ok(entries) = std::fs::read_dir(&at) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let known = if top {
                CARD_FILES.contains(&name.as_str())
                    || name == "tex"
                    || listed(&parts.figures, &name)
            } else {
                listed(&parts.tex, &name)
            };
            if !known {
                return Err(CliError::usage(format!(
                    "report: {} holds a file --card does not write; use an empty folder",
                    e.path().display()
                )));
            }
        }
    }
    Ok(())
}

/// Renders what `--card` writes beyond the card and checks it against `dir` and the study
/// folder.
fn plan_card(home: &Home, card: &Card, dir: &Path) -> Result<Parts, CliError> {
    let usage = |e: String| CliError::usage(format!("report: {e}"));
    let lines = recipe::manifest_lines(&home.study_dir(&card.study)).map_err(usage)?;
    let manifest: String = lines.iter().map(|l| format!("{l}\n")).collect();
    if sha256_hex(manifest.as_bytes()) != card.recipe.manifest.tree_sha256 {
        return Err(usage(
            "the study folder changed while the card was built".into(),
        ));
    }
    let parts = Parts {
        tex: render::latex(card).map_err(usage)?,
        figures: render::figures(card).map_err(usage)?,
        manifest,
    };
    check_folder(dir, &parts)?;
    Ok(parts)
}

/// Writes the card files into `dir`. `readings.md` is written only when absent.
fn write_card(card: &Card, parts: &Parts, dir: &Path) -> std::io::Result<()> {
    let own = [
        ("card.json".to_string(), card.json()),
        ("card.md".to_string(), render::markdown(card)),
        ("manifest.sha256".to_string(), parts.manifest.clone()),
    ];
    write_files(dir, &own)?;
    write_files(dir, &parts.figures)?;
    write_files(&dir.join("tex"), &parts.tex)?;
    let made = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(readings::FILE));
    match made {
        Ok(mut f) => std::io::Write::write_all(&mut f, readings::template(card).as_bytes()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

fn run_card(ctx: &Ctx, id: &str, dir: &Path) -> Run {
    let home = ctx.home()?;
    let (card, parts) = report_with(ctx, id, |card| plan_card(home, card, dir))?;
    write_card(&card, &parts, dir).map_err(|e| {
        CliError::usage(format!(
            "{}: {e}; the opened record (seq {}) was appended",
            dir.display(),
            card.opened.seq
        ))
    })?;
    out(&format!("wrote the card of {id} to {}\n", dir.display()))?;
    Ok(Done::exit(0))
}

pub fn run(ctx: &Ctx, id: &str, format: Format, card: bool, dir: Option<&Path>) -> Run {
    if card {
        let dir = dir.ok_or_else(|| CliError::usage("report: --card needs --out <dir>"))?;
        return run_card(ctx, id, dir);
    }
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
        (Format::Json, _) => r.json(),
        (Format::Md, _) => render::markdown(&r),
        (Format::Tex, Some(dir)) => {
            format!("wrote {} files to {}\n", write_tex(&r, dir)?, dir.display())
        }
        _ => text(&r),
    };
    out(&shown)?;
    Ok(Done::exit(0))
}
