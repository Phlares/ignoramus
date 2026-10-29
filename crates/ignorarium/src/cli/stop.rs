//! `stop`: end a study for good. A live run writes the stop record; an idle study gets it here.
use super::{CliError, Ctx, Done, Run, out, registered};
use crate::control::{ControlFile, stop_if_idle};
use crate::home::Home;
use crate::records::{self, Record};
use ed25519_dalek::SigningKey;
use std::time::{Duration, Instant};

/// How long `stop` waits for a live run to write the record.
const WAIT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(50);

pub enum Stopped {
    Recorded(Record),
    /// The request is saved; the live run has not answered within the wait.
    Requested,
}

/// Send the stop, then wait up to `wait` for the study's stop record.
pub fn stop_study(
    home: &Home,
    key: &SigningKey,
    id: &str,
    reason: &str,
    wait: Duration,
) -> Result<Stopped, CliError> {
    let fault = |e: &dyn std::fmt::Display| CliError::usage(format!("cannot stop {id}: {e}"));
    ControlFile::of(home, id)
        .request_stop(reason)
        .map_err(|e| fault(&e))?;
    let log = home.study_dir(id).join("records.jsonl");
    let start = Instant::now();
    loop {
        if let Some(r) = stop_if_idle(home, id, key).map_err(|e| fault(&e))? {
            return Ok(Stopped::Recorded(r));
        }
        let all = records::read_all(&log, &key.verifying_key()).map_err(|e| fault(&e))?;
        if let Some(r) = all.into_iter().find(|r| r.kind == "stop") {
            return Ok(Stopped::Recorded(r));
        }
        if start.elapsed() >= wait {
            return Ok(Stopped::Requested);
        }
        std::thread::sleep(POLL);
    }
}

pub fn run(ctx: &Ctx, id: &str, reason: &str) -> Run {
    let (home, key, _) = registered(ctx, id)?;
    match stop_study(home, &key, id, reason, WAIT)? {
        Stopped::Recorded(r) => out(&format!("stop record: {}\n", r.seq))?,
        Stopped::Requested => out("stop requested; the live run records it\n")?,
    }
    Ok(Done::exit(0))
}
