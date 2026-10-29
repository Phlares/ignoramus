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
pub mod register;
mod review;
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
    let done = cli.verb.run(&ctx).unwrap_or_else(|e| {
        eprintln!("error: {}", e.msg);
        Done {
            code: e.code,
            spec_hash: None,
            leaf: None,
        }
    });
    if let Err(e) = emit(&ctx, name, &done) {
        eprintln!("warning: cannot write the event log: {e}");
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
