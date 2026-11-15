//! `run`: prepare a registered study and run its rows.
use super::{CliError, Ctx, Done, Run, out, refuse_if_stopped, registered};
use crate::control::{ControlFile, Signal, run_is_live};
use crate::runner::{self, RunError, RunStatus};
use crate::{machine, sys};

pub fn run(ctx: &Ctx, id: &str, max_level: Option<u8>, box_changed: Option<String>) -> Run {
    go(ctx, id, max_level, false, box_changed)
}

/// `run`, or `resume` when `resume` is set: the same refusals, except that `resume`
/// clears the pending pause that `run` refuses on. `box_changed` is the reason for a declared
/// change of box binary; a `resume` that a live run takes ignores it.
pub fn go(
    ctx: &Ctx,
    id: &str,
    max_level: Option<u8>,
    resume: bool,
    box_changed: Option<String>,
) -> Run {
    let (home, _, all) = registered(ctx, id)?;
    refuse_if_stopped(id, &all)?;
    let control = ControlFile::of(home, id);
    // A live run reads the request at its next row boundary; this call starts nothing.
    if resume && run_is_live(home, id).map_err(|e| CliError::usage(e.to_string()))? {
        control
            .request_resume()
            .map_err(|e| CliError::usage(format!("cannot write the resume request: {e}")))?;
        out("the live run continues
")?;
        return Ok(Done::exit(0));
    }
    let pending = control
        .peek()
        .map_err(|e| CliError::usage(format!("cannot read the control file: {e}")))?;
    if !resume && pending == Some(Signal::Pause) {
        return Err(CliError::refuse(format!(
            "a pause is pending; run `ignorarium resume {id}`"
        )));
    }
    let machine = machine::load(&home.root).map_err(|e| CliError::usage(e.to_string()))?;
    let reach = machine.max_level.min(max_level.unwrap_or(u8::MAX));
    let root = crate::inputs::current_root()
        .map_err(|e| CliError::usage(format!("cannot read the current folder: {e}")))?;
    let mut plan = runner::prepare(home, id, &root, reach).map_err(|e| match e {
        // Level 0 always runs, so `have` is never empty.
        RunError::NoBackend { need, have } => CliError::refuse(format!(
            "study {id} needs isolation level {need}; this machine reaches level {}. \
             Run `ignorarium probe` to measure it again",
            have.iter().max().unwrap_or(&0)
        )),
        e => run_error(e),
    })?;
    // The runner sends the resume request once the binary gate passes, so a refused resume
    // leaves the pause pending.
    plan.resumed_by = resume;
    plan.box_changed = box_changed;
    // Rows start below: a terminal signal must end the row, not leave it running.
    sys::catch_interrupts()
        .map_err(|e| CliError::usage(format!("cannot install the signal handlers: {e}")))?;
    let status = runner::run(&plan).map_err(run_error)?;
    match status {
        RunStatus::Done => {
            out(&format!("done: {id}\n"))?;
            Ok(Done {
                spec_hash: Some(format!("sha256:{}", hex::encode(plan.spec_hash))),
                ..Done::exit(0)
            })
        }
        RunStatus::Paused(why) => Err(CliError::refuse(format!("paused: {why}"))),
        RunStatus::Stopped(why) => Err(CliError::refuse(format!("stopped: {why}"))),
    }
}

/// A store or I/O fault is exit 2; anything else is a refusal.
fn run_error(e: RunError) -> CliError {
    match e {
        RunError::Io(_) | RunError::Records(_) => CliError::usage(e.to_string()),
        e => CliError::refuse(e.to_string()),
    }
}
