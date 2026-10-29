//! `pause`: ask a run to stop at the next row boundary.
use super::{CliError, Ctx, Done, Run, out, registered, stopped_why};
use crate::control::ControlFile;

pub fn run(ctx: &Ctx, id: &str) -> Run {
    let (home, _, all) = registered(ctx, id)?;
    if let Some(why) = stopped_why(&all) {
        return Err(CliError::refuse(format!("study {id} is stopped: {why}")));
    }
    ControlFile::of(home, id)
        .request_pause()
        .map_err(|e| CliError::usage(format!("cannot write the pause request: {e}")))?;
    out(&format!("pause requested for {id}\n"))?;
    Ok(Done::exit(0))
}
