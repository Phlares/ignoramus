//! The command line: argument parsing, the shared context, and one event per call.
use crate::events::{self, Event};
use crate::home::Home;
use crate::rules;
use crate::sys;
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::ffi::OsString;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub mod check;
mod docs;
mod guide;
mod history;
mod import;
mod new;
mod pause;
mod probe;
pub mod register;
mod report;
mod resume;
mod review;
mod run;
mod selftest;
pub mod status;
pub mod stop;
mod verify;

/// A verb failed. `code` is the exit code: 1 for a refusal, 2 for a usage or I/O error.
#[derive(Debug)]
pub struct CliError {
    pub code: i32,
    pub msg: String,
}

impl CliError {
    pub fn refuse(msg: impl Into<String>) -> Self {
        CliError {
            code: 1,
            msg: msg.into(),
        }
    }

    pub fn usage(msg: impl Into<String>) -> Self {
        CliError {
            code: 2,
            msg: msg.into(),
        }
    }
}

/// What a verb did: its exit code, the spec hash when it read a spec,
/// and the plan log leaf index when it added one.
#[derive(Debug)]
pub struct Done {
    pub code: i32,
    pub spec_hash: Option<String>,
    pub leaf: Option<u64>,
    /// Extra attributes for the verb's event.
    pub attrs: Option<Value>,
}

impl Done {
    /// A verb that read no spec and added no leaf.
    pub fn exit(code: i32) -> Self {
        Done {
            code,
            spec_hash: None,
            leaf: None,
            attrs: None,
        }
    }
}

type Run = Result<Done, CliError>;

/// State shared by the verbs.
pub struct Ctx {
    home: Result<Home, String>,
}

impl Ctx {
    fn home(&self) -> Result<&Home, CliError> {
        self.home.as_ref().map_err(|e| CliError::usage(e.clone()))
    }
}

/// A reason goes into a signed record, so it must say something. The value stays as typed.
fn non_blank(s: &str) -> Result<String, String> {
    if s.trim().is_empty() {
        return Err("the reason must not be blank".into());
    }
    Ok(s.to_owned())
}

/// Write to stdout. A closed pipe is not an error; any other failure is.
fn out(text: &str) -> Result<(), CliError> {
    let mut stdout = std::io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => {
            Err(CliError::usage(format!("cannot write to stdout: {e}")))
        }
        _ => Ok(()),
    }
}

#[derive(Parser)]
#[command(version, about = "Checks experiment plans before they run.")]
struct Cli {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Subcommand)]
enum Verb {
    /// Print the steps of a study, in order.
    Guide {
        /// Print the steps as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Copy a template into studies/<id>/spec.toml under the current folder.
    New {
        /// Template name, for example numeric-ablation.
        #[arg(long)]
        template: String,
        /// Study id: ASCII lowercase letters and digits joined by single hyphens.
        #[arg(long)]
        id: String,
    },
    /// Check a spec against every rule and print the cost estimate.
    Check {
        /// The spec file.
        spec: PathBuf,
        /// Print one JSON object instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Print one Markdown page for a person to read before approving a plan.
    Review {
        /// The spec file.
        spec: PathBuf,
    },
    /// Approve a checked plan and add it to the plan log. Needs a terminal.
    Register {
        /// The spec file.
        spec: PathBuf,
    },
    /// Record the hashes of earlier data files, so later plans cannot reuse them.
    ImportHistory {
        /// Name of the earlier phase, kept as the leaf label. Must not start with `study:`.
        #[arg(long)]
        phase: String,
        /// The data files.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Add files to the box's content store, so an input can name one by its digest.
    /// Importing a file again adds another signed record.
    Import {
        /// Where the files came from, kept in the signed record.
        #[arg(long)]
        source: String,
        /// The files.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Check the plan logs and, with --study, one study's record chain.
    Verify {
        /// Study id.
        #[arg(long)]
        study: Option<String>,
    },
    /// Measure this machine and write machine.toml. Run it before `check`.
    Probe,
    /// Run a registered study. Rows with a signed record are skipped.
    Run {
        /// Study id.
        study: String,
        /// Use no backend above this isolation level.
        #[arg(long, value_parser = clap::value_parser!(u8).range(..=3))]
        max_level: Option<u8>,
        /// Run on a box binary that differs from the last session's, for this reason. The
        /// reason goes into the deviation log and the session record.
        #[arg(long, value_name = "REASON", value_parser = non_blank)]
        box_changed: Option<String>,
    },
    /// Show how far a study has run and what it has spent.
    Status {
        /// Study id.
        study: String,
        /// Print one JSON object instead of text.
        #[arg(long)]
        json: bool,
        /// Show the host readings of the rows, with no measured value.
        #[arg(long)]
        telemetry: bool,
    },
    /// Print the verdicts of a finished study.
    Report {
        /// Study id.
        study: String,
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: report::Format,
        /// Same as `--format json`.
        #[arg(long, conflicts_with = "format")]
        json: bool,
        /// Write the frozen card, its figures, manifest and author slots to `--out`.
        #[arg(long, conflicts_with_all = ["format", "json"], requires = "out")]
        card: bool,
        /// The folder `--format tex` or `--card` writes its files to.
        #[arg(long)]
        out: Option<PathBuf>,
        /// The author's git folder of the study. The card hashes its outside files there and
        /// pins them by commit.
        #[arg(long)]
        study_folder: Option<PathBuf>,
    },
    /// Ask a run to pause at the next row boundary.
    Pause {
        /// Study id.
        study: String,
    },
    /// Clear a pause and run the remaining rows.
    Resume {
        /// Study id.
        study: String,
        /// Resume on a box binary that differs from the last session's, for this reason. The
        /// reason goes into the deviation log and the session record. Ignored when a live run
        /// takes the resume request.
        #[arg(long, value_name = "REASON", value_parser = non_blank)]
        box_changed: Option<String>,
    },
    /// End a study for good and record why. Works on a paused or idle study.
    Stop {
        /// Study id.
        study: String,
        /// Why the study stops.
        #[arg(long, value_name = "REASON", value_parser = non_blank)]
        reason: String,
    },
    #[command(hide = true, subcommand)]
    Dev(Dev),
}

#[derive(Subcommand)]
enum Dev {
    /// Write docs/rules/<code>.md from the built-in rules.
    GenRuleDocs {
        /// Folder to write into.
        #[arg(long, default_value = "docs/rules")]
        out: PathBuf,
    },
    /// Check the sandbox this runs in. Prints one JSON line; exits 102 when it fails.
    Selftest,
}

impl Verb {
    /// The study id of a verb that takes one.
    fn study(&self) -> Option<&str> {
        match self {
            Verb::Run { study, .. }
            | Verb::Status { study, .. }
            | Verb::Report { study, .. }
            | Verb::Pause { study }
            | Verb::Resume { study, .. }
            | Verb::Stop { study, .. } => Some(study),
            Verb::Verify { study } => study.as_deref(),
            _ => None,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Verb::Guide { .. } => "guide",
            Verb::New { .. } => "new",
            Verb::Check { .. } => "check",
            Verb::Review { .. } => "review",
            Verb::Register { .. } => "register",
            Verb::ImportHistory { .. } => "import-history",
            Verb::Import { .. } => "import",
            Verb::Verify { .. } => "verify",
            Verb::Probe => "probe",
            Verb::Run { .. } => "run",
            Verb::Status { .. } => "status",
            Verb::Report { .. } => "report",
            Verb::Pause { .. } => "pause",
            Verb::Resume { .. } => "resume",
            Verb::Stop { .. } => "stop",
            Verb::Dev(Dev::GenRuleDocs { .. }) => "dev.gen-rule-docs",
            Verb::Dev(Dev::Selftest) => "dev.selftest",
        }
    }

    fn run(self, ctx: &Ctx) -> Run {
        match self {
            Verb::Guide { json } => guide::run(json),
            Verb::New { template, id } => new::run(&template, &id),
            Verb::Check { spec, json } => check::run(ctx, &spec, json),
            Verb::Review { spec } => review::run(ctx, &spec),
            Verb::Register { spec } => register::run_stdio(ctx, &spec),
            Verb::ImportHistory { phase, files } => history::run(ctx, &phase, &files),
            Verb::Import { source, files } => import::run(ctx, &source, &files),
            Verb::Verify { study } => verify::run(ctx, study.as_deref()),
            Verb::Probe => probe::run(ctx),
            Verb::Run {
                study,
                max_level,
                box_changed,
            } => run::run(ctx, &study, max_level, box_changed),
            Verb::Status {
                study,
                json,
                telemetry,
            } => status::run(ctx, &study, json, telemetry),
            Verb::Report {
                study,
                format,
                json,
                card,
                out,
                study_folder,
            } => report::run(
                ctx,
                &study,
                if json { report::Format::Json } else { format },
                card,
                out.as_deref(),
                study_folder.as_deref(),
            ),
            Verb::Pause { study } => pause::run(ctx, &study),
            Verb::Resume { study, box_changed } => resume::run(ctx, &study, box_changed),
            Verb::Stop { study, reason } => stop::run(ctx, &study, &reason),
            Verb::Dev(Dev::GenRuleDocs { out }) => docs::run(&out),
            Verb::Dev(Dev::Selftest) => selftest::run(),
        }
    }
}

/// Run one call and return the exit code.
pub fn run<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(c) => c,
        Err(e) => {
            let _ = e.print();
            return e.exit_code();
        }
    };
    let name = cli.verb.name();
    let started = Instant::now();
    let trace_id = events::new_trace_id();
    let span_id = events::new_span_id();
    let ctx = Ctx {
        home: Home::open().map_err(|e| e.to_string()),
    };
    // The selftest runs in a sandbox where the event log is out of reach. A box home that
    // cannot be opened warns once, for the call, not for each event.
    let log = match &ctx.home {
        _ if name == "dev.selftest" => None,
        Ok(home) => Some(home.root.join("events.jsonl")),
        Err(e) => {
            let _ = writeln!(
                std::io::stderr(),
                "warning: cannot write the event log: {e}"
            );
            None
        }
    };
    if let Some(log) = &log {
        let mut attrs = json!({
            "verb": name,
            "pid": std::process::id(),
            "parent": sys::parent_name(),
            "stdin_tty": std::io::stdin().is_terminal(),
        });
        if let (Some(map), Some(study)) = (attrs.as_object_mut(), cli.verb.study()) {
            map.insert("study".into(), study.into());
        }
        warn_on_err(write_event(log, &trace_id, &span_id, "call.start", attrs));
    }
    // A hung-up terminal fails these writes; the exit code and the event still stand.
    let done = cli.verb.run(&ctx).unwrap_or_else(|e| {
        let _ = writeln!(std::io::stderr(), "error: {}", e.msg);
        Done::exit(e.code)
    });
    if let Some(log) = &log {
        let attrs = end_attrs(&done, started);
        warn_on_err(write_event(log, &trace_id, &span_id, name, attrs));
    }
    done.code
}

fn warn_on_err(r: Result<(), String>) {
    if let Err(e) = r {
        let _ = writeln!(
            std::io::stderr(),
            "warning: cannot write the event log: {e}"
        );
    }
}

fn end_attrs(done: &Done, started: Instant) -> Value {
    let mut attrs = json!({
        "exit": done.code,
        "duration_ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "peak_rss_kib": sys::peak_rss_kib(),
    });
    if let Some(map) = attrs.as_object_mut() {
        if let Some(h) = &done.spec_hash {
            map.insert("spec_hash".into(), Value::String(h.clone()));
        }
        if let Some(i) = done.leaf {
            map.insert("leaf".into(), Value::from(i));
        }
        if let Some(Value::Object(extra)) = &done.attrs {
            map.extend(extra.clone());
        }
    }
    attrs
}

fn write_event(
    log: &Path,
    trace_id: &str,
    span_id: &str,
    name: &str,
    attrs: Value,
) -> Result<(), String> {
    events::emit(
        log,
        Event {
            ts: crate::time::now_rfc3339(),
            trace_id: trace_id.into(),
            span_id: Some(span_id.into()),
            name: name.into(),
            level: "info".into(),
            attrs,
        },
    )
    .map_err(|e| e.to_string())
}

/// A study with a record log, before the log is read: the box home and the path of the log.
/// Refuses an id that is not a study id, and a study with no log.
fn located<'a>(ctx: &'a Ctx, id: &str) -> Result<(&'a Home, PathBuf), CliError> {
    let home = ctx.home()?;
    if !new::id_is_valid(id)? {
        return Err(CliError::refuse(format!("{id} is not a study id")));
    }
    let log = home.study_dir(id).join("records.jsonl");
    if !log.is_file() {
        return Err(not_registered(id));
    }
    Ok((home, log))
}

pub(crate) fn not_registered(id: &str) -> CliError {
    CliError::refuse(format!("study {id} is not registered"))
}

/// A registered study: the box key and the study's verified records.
/// Refuses what `located` refuses, and a study with no `register` record.
fn registered<'a>(
    ctx: &'a Ctx,
    id: &str,
) -> Result<
    (
        &'a Home,
        ed25519_dalek::SigningKey,
        Vec<crate::records::Record>,
    ),
    CliError,
> {
    let (home, log) = located(ctx, id)?;
    let key = crate::keys::load(home)
        .map_err(|e| CliError::usage(format!("box key: {e}")))?
        .ok_or_else(|| not_registered(id))?;
    let all = crate::records::read_all(&log, &key.verifying_key())
        .map_err(|e| CliError::usage(format!("records of {id}: {e}")))?;
    if !all
        .iter()
        .any(|r| crate::derived::Register::of(r).is_some())
    {
        return Err(not_registered(id));
    }
    Ok((home, key, all))
}

/// A refusal for a code that only the command line raises; the text comes from the rule
/// file. `found` says what this case found, when the rule text alone does not.
pub fn refusal(code: &str, found: &str) -> CliError {
    let text = rules::builtin()
        .iter()
        .find(|r| r.code == code)
        .map_or(String::new(), |r| {
            format!("{} {}", r.problem, r.instruction)
        });
    let found = if found.is_empty() {
        String::new()
    } else {
        format!(" Found: {found}.")
    };
    CliError::refuse(format!("{code}: {text}{found} See docs/rules/{code}.md"))
}

/// Refuse a study whose log holds a `done` record (E-DON-001).
pub fn refuse_if_done(id: &str, all: &[crate::records::Record]) -> Result<(), CliError> {
    match all.iter().find(|r| r.kind == crate::derived::DONE) {
        Some(r) => Err(refusal(
            "E-DON-001",
            &format!("study {id} has a done record, seq {}", r.seq),
        )),
        None => Ok(()),
    }
}

/// Refuse a study that has a stop record.
fn refuse_if_stopped(id: &str, all: &[crate::records::Record]) -> Result<(), CliError> {
    match stopped_why(all) {
        Some(why) => Err(CliError::refuse(format!("study {id} is stopped: {why}"))),
        None => Ok(()),
    }
}

/// The reason in the study's stop record, when it has one.
pub(crate) fn stopped_why(all: &[crate::records::Record]) -> Option<String> {
    all.iter()
        .find(|r| r.kind == "stop")
        .map(|r| r.body["why"].as_str().unwrap_or_default().to_string())
}
