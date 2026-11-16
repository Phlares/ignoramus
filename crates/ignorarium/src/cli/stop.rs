//! `stop`: end a study for good. A live run writes the stop record; an idle study gets it here.
use super::{CliError, Ctx, Done, Run, located, out, refuse_if_done, registered, stopped_why};
use crate::control::{ControlError, ControlFile, await_stop};
use crate::derived::first_done;
use crate::records;
use std::io::Write;
use std::time::Duration;

/// How long `stop` waits for a live run to write the record.
const STOP_WAIT: Duration = Duration::from_secs(30);

/// A log that reads and holds `done` refuses before anything is written. A log that cannot be
/// read still gets the request, so a torn log cannot keep a live run from seeing it. "The first
/// stop's reason stands" is printed only when the log held a stop record before this call.
pub fn run(ctx: &Ctx, id: &str, reason: &str) -> Run {
    let (home, log) = located(ctx, id)?;
    let fault = |e: &dyn std::fmt::Display| CliError::usage(format!("cannot stop {id}: {e}"));
    let read = registered(ctx, id);
    if let Ok((_, _, all)) = &read {
        refuse_if_done(id, all)?;
    }
    ControlFile::of(home, id)
        .request_stop(reason)
        .map_err(|e| fault(&e))?;
    let (home, key, all) = read.map_err(|e| CliError {
        msg: format!("the stop request is sent, but {}", e.msg),
        ..e
    })?;
    match await_stop(home, id, &key, STOP_WAIT) {
        Ok(Some(s)) => {
            out(&format!("stop record: {}\n", s.record.seq))?;
            if let Some(e) = s.close_error {
                let _ = writeln!(
                    std::io::stderr(),
                    "the done record is not written: {e}; run stop again"
                );
            }
        }
        Ok(None) => out("stop requested; the live run records it\n")?,
        // A run wrote `done` after the log was read; the request is written by then.
        Err(ControlError::Done) => {
            return Err(CliError::refuse(
                "the stop request is sent, but the study ended first; the request is cleared",
            ));
        }
        Err(e) => return Err(fault(&e)),
    }
    if stopped_why(&all).is_some() {
        out("the first stop's reason stands\n")?;
        let now = records::read_all(&log, &key.verifying_key()).map_err(|e| fault(&e))?;
        if let Some(done) = first_done(&now) {
            out(&format!("done record: {}\n", done.seq))?;
        }
    }
    Ok(Done::exit(0))
}
