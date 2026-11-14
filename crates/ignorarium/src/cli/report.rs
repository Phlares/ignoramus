//! `report`: the verdicts of a finished study, with the clip counts of its mean claims.
//! Appends one `opened` record, so a reader of the log sees that the results were read.
use super::{CliError, Ctx, Done, Run, located, not_registered, out};
use crate::canon::sha256_hex;
use crate::card::sidecar::{DATA_FILE, OBSERVATIONS_FILE, Sidecars};
use crate::card::{self, Card, Opened, readings, recipe, render};
use crate::derived::OPENED;
use crate::diag::printable;
use crate::files::optional;
use crate::home::Home;
use crate::records::{self, Log, RecordDraft, RecordError};
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

pub fn report(ctx: &Ctx, id: &str, folder: Option<&Path>) -> Result<Card, CliError> {
    report_with(ctx, id, folder, |_, _| Ok(())).map(|(card, ())| card)
}

/// `report`, with `before` run on the built card and its log ahead of the `opened` append. When
/// `before` refuses, nothing is appended.
fn report_with<T>(
    ctx: &Ctx,
    id: &str,
    folder: Option<&Path>,
    before: impl FnOnce(&Card, &Log) -> Result<T, CliError>,
) -> Result<(Card, T), CliError> {
    let (home, log) = located(ctx, id)?;
    let key = crate::keys::load(home)
        .map_err(|e| CliError::usage(format!("box key: {e}")))?
        .ok_or_else(|| not_registered(id))?;
    let mut built = None;
    let appended = records::append_with(&log, &key, |all| -> Result<_, Stop> {
        let card = card::build(home, id, all, folder)?;
        built = Some((before(&card, all)?, card));
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
        "study {}: finished {}, {} records verified\n",
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

fn latex(card: &Card) -> Result<Vec<(String, String)>, CliError> {
    render::latex(card).map_err(|e| CliError::usage(format!("report: {e}")))
}

const CARD_FILES: [&str; 6] = [
    "card.json",
    "card.md",
    "manifest.sha256",
    DATA_FILE,
    OBSERVATIONS_FILE,
    readings::FILE,
];

/// What `--card` writes besides `card.json`, `card.md` and `readings.md`. Rendered and checked
/// before the `opened` append.
struct Parts {
    tex: Vec<(String, String)>,
    figures: Vec<(String, String)>,
    manifest: String,
    sidecars: Sidecars,
}

/// Refuses an `--out` folder that holds a symlink, a file this command does not write, or
/// anything `read_dir` cannot list. Checked at every depth with `symlink_metadata`.
fn check_folder(dir: &Path, parts: &Parts) -> Result<(), CliError> {
    let refuse = |at: &Path, why: &str| {
        CliError::usage(format!(
            "report: {}: {why}; use an empty folder",
            at.display()
        ))
    };
    let listed = |files: &[(String, String)], name: &str| files.iter().any(|(n, _)| n == name);
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(refuse(dir, &e.to_string())),
        Ok(m) if m.file_type().is_symlink() => return Err(refuse(dir, "a symlink")),
        Ok(m) if !m.is_dir() => return Err(refuse(dir, "not a folder")),
        Ok(_) => {}
    }
    let mut pending = vec![(dir.to_path_buf(), 0)];
    while let Some((at, depth)) = pending.pop() {
        let entries = std::fs::read_dir(&at).map_err(|e| refuse(&at, &e.to_string()))?;
        for e in entries {
            let e = e.map_err(|e| refuse(&at, &e.to_string()))?;
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let kind = (std::fs::symlink_metadata(&path))
                .map_err(|e| refuse(&path, &e.to_string()))?
                .file_type();
            let known = match (depth, kind.is_dir()) {
                _ if kind.is_symlink() => return Err(refuse(&path, "a symlink")),
                (0, true) => name == "tex",
                (0, false) => CARD_FILES.contains(&name.as_str()) || listed(&parts.figures, &name),
                (_, true) => false,
                (_, false) => listed(&parts.tex, &name),
            };
            if !known {
                return Err(refuse(&path, "a file --card does not write"));
            }
            if kind.is_dir() {
                pending.push((path, depth + 1));
            }
        }
    }
    Ok(())
}

/// Renders what `--card` writes beyond the card and checks it against `dir` and the study
/// folder.
fn plan_card(home: &Home, card: &Card, log: &Log, dir: &Path) -> Result<Parts, CliError> {
    let usage = |e: String| CliError::usage(format!("report: {e}"));
    let lines = recipe::manifest_lines(&home.study_dir(&card.study)).map_err(usage)?;
    let manifest = recipe::list_text(&lines);
    let sidecars = Sidecars::of(home, &card.study, log).map_err(usage)?;
    let unchanged = sha256_hex(manifest.as_bytes()) == card.recipe.manifest.list_sha256
        && sidecars.data_list() == card.setup.data
        && sidecars.observations_card(card.observations.forced.clone()) == card.observations;
    if !unchanged {
        return Err(usage(
            "the study folder changed while the card was built".into(),
        ));
    }
    let parts = Parts {
        sidecars,
        tex: latex(card)?,
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
        (DATA_FILE.to_string(), parts.sidecars.data.clone()),
        (
            OBSERVATIONS_FILE.to_string(),
            parts.sidecars.observations.clone(),
        ),
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

/// Prints a flag on stderr for each way the existing `readings.md` does not fit the new card.
fn flag_readings(old: &[u8], card: &Card) {
    let text = String::from_utf8_lossy(old);
    let now = card.sha256();
    match readings::header_hash(&text) {
        Some(h) if h == now => {}
        found => eprintln!(
            "report: {} names card {}, but the new card is {now}",
            readings::FILE,
            found.unwrap_or("none")
        ),
    }
    // Text from `readings.md` is the author's: escaped and cut before it reaches a terminal.
    let shown = |t: &str| printable(&t.chars().take(80).collect::<String>());
    for e in readings::check(&text) {
        eprintln!(
            "report: {} line {} ({}): {}",
            readings::FILE,
            e.line,
            shown(&e.slot),
            shown(&e.excerpt)
        );
    }
}

fn run_card(ctx: &Ctx, id: &str, dir: &Path, folder: Option<&Path>) -> Run {
    let home = ctx.home()?;
    let (card, parts) = report_with(ctx, id, folder, |card, log| plan_card(home, card, log, dir))?;
    let old = optional(std::fs::read(dir.join(readings::FILE))).unwrap_or_else(|e| {
        eprintln!("report: cannot read {} to check it: {e}", readings::FILE);
        None
    });
    write_card(&card, &parts, dir).map_err(|e| {
        CliError::usage(format!(
            "{}: {e}; the opened record (seq {}) was appended",
            dir.display(),
            card.opened.seq
        ))
    })?;
    if let Some(old) = old {
        flag_readings(&old, &card);
    }
    out(&format!("wrote the card of {id} to {}\n", dir.display()))?;
    Ok(Done::exit(0))
}

pub fn run(
    ctx: &Ctx,
    id: &str,
    format: Format,
    card: bool,
    dir: Option<&Path>,
    folder: Option<&Path>,
) -> Run {
    if card {
        let dir = dir.ok_or_else(|| CliError::usage("report: --card needs --out <dir>"))?;
        return run_card(ctx, id, dir, folder);
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
    let shown = match (format, dir) {
        (Format::Tex, Some(dir)) => {
            let (_, files) = report_with(ctx, id, folder, |card, _| latex(card))?;
            write_files(dir, &files)
                .map_err(|e| CliError::usage(format!("{}: {e}", dir.display())))?;
            format!("wrote {} files to {}\n", files.len(), dir.display())
        }
        (Format::Json, _) => report(ctx, id, folder)?.json(),
        (Format::Md, _) => render::markdown(&report(ctx, id, folder)?),
        _ => text(&report(ctx, id, folder)?),
    };
    out(&shown)?;
    Ok(Done::exit(0))
}
