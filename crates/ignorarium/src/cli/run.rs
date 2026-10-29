//! `run`: prepare a registered study and run its rows.
use super::{CliError, Ctx, Done, Run, out, refuse_if_stopped, registered};
use crate::control::{ControlFile, Signal, run_is_live};
use crate::machine;
use crate::runner::{self, RunError, RunStatus};

pub fn run(ctx: &Ctx, id: &str, max_level: Option<u8>) -> Run {
    go(ctx, id, max_level, false)
}

/// `run`, or `resume` when `resume` is set: the same refusals, except that `resume`
/// clears the pending pause that `run` refuses on.
pub fn go(ctx: &Ctx, id: &str, max_level: Option<u8>, resume: bool) -> Run {
    let (home, _, all) = registered(ctx, id)?;
    refuse_if_stopped(id, &all)?;
    let control = ControlFile::of(home, id);
    let request_resume = || {
        control
            .request_resume()
            .map_err(|e| CliError::usage(format!("cannot write the resume request: {e}")))
    };
    // A live run reads the request at its next row boundary; this call starts nothing.
    if resume && run_is_live(home, id).map_err(|e| CliError::usage(e.to_string()))? {
        request_resume()?;
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
    let plan = runner::prepare(home, id, reach).map_err(|e| match e {
        RunError::NoBackend { need, .. } => CliError::refuse(format!(
            "study {id} needs isolation level {need}; this machine reaches level {reach}. \
             Run `ignorarium probe` to measure it again"
        )),
        e => run_error(e),
    })?;
    if resume {
        request_resume()?;
    }
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
