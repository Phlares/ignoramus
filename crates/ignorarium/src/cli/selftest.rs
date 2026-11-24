//! `dev selftest`: the check a level 1 sandbox runs on itself. It reads and writes nothing
//! of the box's, since the sandbox hides the box home.
use super::exit::Code;
use super::{CliError, Done, Run, out};
use crate::backend::selftest;

pub fn run() -> Run {
    let t = selftest::run();
    let line = serde_json::to_string(&t).map_err(|e| CliError::usage(e.to_string()))?;
    out(&format!("{line}\n"))?;
    Ok(Done::exit(if t.ok() {
        Code::Done
    } else {
        Code::SelftestFailed
    }))
}
