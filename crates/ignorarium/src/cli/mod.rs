//! The command line: argument parsing, the shared context, and one event per call.
use crate::events::{self, Event};
use crate::home::Home;
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

mod check;
mod docs;
mod guide;
mod history;
mod new;
mod pause;
mod probe;
pub mod register;
mod resume;
mod review;
mod run;
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
        /// Name of the earlier phase, kept as the leaf label.
        #[arg(long)]
        phase: String,
        /// The data files.
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
    },
    /// Show how far a study has run and what it has spent.
    Status {
        /// Study id.
        study: String,
        /// Print one JSON object instead of text.
        #[arg(long)]
        json: bool,
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
    },
    /// End a study for good and record why. Works on a paused or idle study.
    Stop {
        /// Study id.
        study: String,
        /// Why the study stops.
        #[arg(long)]
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
}

impl Verb {
    fn name(&self) -> &'static str {
        match self {
            Verb::Guide { .. } => "guide",
            Verb::New { .. } => "new",
            Verb::Check { .. } => "check",
            Verb::Review { .. } => "review",
            Verb::Register { .. } => "register",
            Verb::ImportHistory { .. } => "import-history",
            Verb::Verify { .. } => "verify",
            Verb::Probe => "probe",
            Verb::Run { .. } => "run",
            Verb::Status { .. } => "status",
            Verb::Pause { .. } => "pause",
            Verb::Resume { .. } => "resume",
            Verb::Stop { .. } => "stop",
            Verb::Dev(Dev::GenRuleDocs { .. }) => "dev.gen-rule-docs",
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
            Verb::Verify { study } => verify::run(ctx, study.as_deref()),
            Verb::Probe => probe::run(ctx),
            Verb::Run { study, max_level } => run::run(ctx, &study, max_level),
            Verb::Status { study, json } => status::run(ctx, &study, json),
            Verb::Pause { study } => pause::run(ctx, &study),
            Verb::Resume { study } => resume::run(ctx, &study),
            Verb::Stop { study, reason } => stop::run(ctx, &study, &reason),
            Verb::Dev(Dev::GenRuleDocs { out }) => docs::run(&out),
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
    let ctx = Ctx {
        home: Home::open().map_err(|e| e.to_string()),
    };
    // A hung-up terminal fails these writes; the exit code and the event still stand.
    let done = cli.verb.run(&ctx).unwrap_or_else(|e| {
        let _ = writeln!(std::io::stderr(), "error: {}", e.msg);
        Done::exit(e.code)
    });
    if let Err(e) = emit(&ctx, name, &done) {
        let _ = writeln!(
            std::io::stderr(),
            "warning: cannot write the event log: {e}"
        );
    }
    done.code
}

fn emit(ctx: &Ctx, name: &str, done: &Done) -> Result<(), String> {
    let home = ctx.home.as_ref().map_err(String::clone)?;
    let mut attrs = json!({ "exit": done.code });
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
    events::emit(
        &home.root.join("events.jsonl"),
        Event {
            ts: crate::time::now_rfc3339(),
            trace_id: events::new_trace_id(),
            span_id: None,
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

fn not_registered(id: &str) -> CliError {
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
    if !all.iter().any(|r| r.kind == "register") {
        return Err(not_registered(id));
    }
    Ok((home, key, all))
}

/// Refuse a study that has a stop record.
fn refuse_if_stopped(id: &str, all: &[crate::records::Record]) -> Result<(), CliError> {
    match stopped_why(all) {
        Some(why) => Err(CliError::refuse(format!("study {id} is stopped: {why}"))),
        None => Ok(()),
    }
}

/// The reason in the study's stop record, when it has one.
fn stopped_why(all: &[crate::records::Record]) -> Option<String> {
    all.iter()
        .find(|r| r.kind == "stop")
        .map(|r| r.body["why"].as_str().unwrap_or_default().to_string())
}
