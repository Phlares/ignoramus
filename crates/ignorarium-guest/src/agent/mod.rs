//! The guest agent's steps, written as plain functions so they run without a virtual machine.
pub mod payload;
pub mod selftest;
pub mod steps;

use crate::proto::{Report, SelfTest};
use payload::Ran;
use std::io;

/// Runs the payload only when the self-test passed. `run` returns the run and its output hash.
pub fn gate(
    selftest: SelfTest,
    clock_unix_nanos: u64,
    run: impl FnOnce() -> io::Result<(Ran, String)>,
) -> io::Result<Report> {
    let (end, payload_wall_ns, cpu_seconds, output_hash) = if selftest.ok() {
        let (ran, hash) = run()?;
        (
            Some(ran.end),
            ran.wall.as_nanos() as u64,
            ran.cpu_seconds,
            hash,
        )
    } else {
        (None, 0, 0.0, String::new())
    };
    Ok(Report {
        selftest,
        clock_unix_nanos,
        end,
        payload_wall_ns,
        cpu_seconds,
        output_hash,
    })
}
