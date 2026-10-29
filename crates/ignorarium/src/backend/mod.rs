//! Backends run one schedule row in isolation and report what happened.
pub mod level0;

use std::io;
use std::path::PathBuf;
use std::time::Duration;

/// What to run: a read-only input dir and one write-only output dir.
#[derive(Debug, Clone)]
pub struct RowJob {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub input_dir: PathBuf,
    pub output_dir: PathBuf,
    pub seed: [u8; 32],
    pub wall_timeout: Duration,
    /// Root file system to run in. Level 0 ignores it.
    pub rootfs: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exit {
    Ok,
    /// The payload failed (non-zero exit, signal or timeout). Never retried.
    Payload(i32),
    /// The box or the sandbox failed. Retryable.
    Infra(String),
    /// The watch said stop; the row's process group was killed.
    Stopped,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RowOutcome {
    pub exit: Exit,
    pub cpu_seconds: f64,
    pub wall_seconds: f64,
    pub output_hash: String,
    pub selftest_ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    Available,
    Unavailable(String),
}

/// What a backend tells the box while a row runs.
pub trait Watch {
    /// Called over and over while the row runs. `false` means stop the row now.
    fn tick(&self) -> bool;
    /// The row now runs as process `pid`, the leader of its own process group.
    fn started(&self, _pid: u32) {}
}

impl<F: Fn() -> bool> Watch for F {
    fn tick(&self) -> bool {
        self()
    }
}

pub trait Backend {
    fn level(&self) -> u8;
    fn probe(&self) -> Probe;
    fn run_row(&self, job: &RowJob, watch: &dyn Watch) -> io::Result<RowOutcome>;
}

/// Every backend this build knows, lowest level first.
pub fn registered() -> Vec<Box<dyn Backend>> {
    vec![Box::new(level0::Level0)]
}
