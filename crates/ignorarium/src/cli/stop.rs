//! `stop`: end a study for good. A live run writes the stop record; an idle study gets it here.
use super::{CliError, Ctx, Done, Run, located, out, registered};
use crate::control::{ControlFile, await_stop};
use std::time::Duration;

/// How long `stop` waits for a live run to write the record.
const STOP_WAIT: Duration = Duration::from_secs(30);

/// The request goes in before the record log is read, so a torn log cannot keep a live run
/// from seeing it.
pub fn run(ctx: &Ctx, id: &str, reason: &str) -> Run {
    let (home, _) = located(ctx, id)?;
    let fault = |e: &dyn std::fmt::Display| CliError::usage(format!("cannot stop {id}: {e}"));
    ControlFile::of(home, id)
        .request_stop(reason)
        .map_err(|e| fault(&e))?;
    let (home, key, _) = registered(ctx, id).map_err(|e| CliError {
        msg: format!("the stop request is sent, but {}", e.msg),
        ..e
    })?;
    match await_stop(home, id, &key, STOP_WAIT).map_err(|e| fault(&e))? {
        Some(r) => out(&format!("stop record: {}\n", r.seq))?,
        None => out("stop requested; the live run records it\n")?,
    }
    Ok(Done::exit(0))
}
