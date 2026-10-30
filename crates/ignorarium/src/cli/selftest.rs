//! `dev selftest`: the check a level 1 sandbox runs on itself. It reads and writes nothing
//! of the box's, since the sandbox hides the box home.
use super::{CliError, Done, Run, out};
use crate::backend::selftest;

/// Exit code when a check fails, apart from the code 1 to 3 range of the box's own faults.
const FAILED: i32 = 102;

pub fn run() -> Run {
    let t = selftest::run();
    let line = serde_json::to_string(&t).map_err(|e| CliError::usage(e.to_string()))?;
    out(&format!("{line}\n"))?;
    Ok(Done::exit(if t.ok() { 0 } else { FAILED }))
}
