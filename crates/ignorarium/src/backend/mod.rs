//! Backends run one schedule row in isolation and report what happened.
pub mod level0;
pub mod level1;
pub mod level2;
pub(crate) mod output;
pub mod rootfs;
pub mod seccomp;
pub mod selftest;

pub use output::output_hash;

use crate::sys::{self, Ended};
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// Exit code reported for a row killed at its wall timeout.
pub const TIMEOUT_CODE: i32 = 124;
const POLL: Duration = Duration::from_millis(5);

/// What to run: a read-only input dir and one write-only output dir.
/// The backend sets `IGNORARIUM_IN` and `IGNORARIUM_OUT` to the paths the payload sees.
#[derive(Debug, Clone, Default)]
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
    /// The payload exited 0, but its output cannot be hashed: it removed the folder, made it
    /// a link, or left a file the box cannot read. Never retried.
    Void(String),
    /// The box or the sandbox failed before the payload could run or be reaped. Retryable.
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
    /// What the level 1 self-test found before the row; `None` at level 0 and when it
    /// printed nothing readable.
    pub selftest: Option<selftest::SelfTest>,
    /// Why the CPU count is missing the sandbox's own processes, when it is.
    pub cpu_unknown: Option<String>,
    /// The box ended the row: the watch said stop, or the wall timeout passed. The CPU
    /// count then misses what the killed processes used.
    pub ended: bool,
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
    /// A supervised process group now runs, led by `pid`. Called once for every group, in
    /// order: a level 1 row starts its self-test group and then its own. Each call replaces
    /// the last; the earlier group is already killed and reaped.
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
    /// What starts the payload, recorded beside the isolation level.
    fn launcher(&self) -> String;
    /// How many CPUs a row may use at once.
    fn cpus(&self) -> u32;
    fn run_row(&self, job: &RowJob, watch: &dyn Watch) -> io::Result<RowOutcome>;
}

/// Every backend this build knows, lowest level first.
pub fn registered() -> Vec<Box<dyn Backend>> {
    vec![
        Box::new(level0::Level0),
        Box::new(level1::Level1::new()),
        Box::new(level2::Level2::new()),
    ]
}

/// The CPUs this host gives a row: what the OS reports, 1 when it cannot.
pub(crate) fn host_cpus() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| u32::try_from(n.get()).unwrap_or(u32::MAX))
}

/// How `supervise` ended a group.
#[derive(Debug, Clone, PartialEq)]
pub struct Supervised {
    pub exit: Exit,
    pub cpu_seconds: f64,
    /// The box killed the group: a watch stop or the wall timeout.
    pub ended: bool,
}

impl Supervised {
    fn failed(why: String) -> Supervised {
        Supervised {
            exit: Exit::Infra(why),
            cpu_seconds: 0.0,
            ended: false,
        }
    }
}

/// The outcome of a row that ended with `exit` after `cpu_seconds` of CPU: the wall time
/// since `start` and the hash of `job.output_dir`. `ended` is true when the box killed it.
pub fn outcome(
    job: &RowJob,
    start: Instant,
    mut exit: Exit,
    cpu_seconds: f64,
    ended: bool,
) -> RowOutcome {
    let wall_seconds = start.elapsed().as_secs_f64();
    // A row the box ended is not data: nothing reads its hash, and a large output would
    // hold up the spend record. Otherwise the payload ran, and an output it left
    // unhashable is its own result, not a box fault.
    let output_hash = if exit == Exit::Stopped {
        String::new()
    } else {
        output_hash(&job.output_dir).unwrap_or_else(|e| {
            if exit == Exit::Ok {
                exit = Exit::Void(format!("cannot hash output: {e}"));
            }
            String::new()
        })
    };
    RowOutcome {
        exit,
        cpu_seconds,
        wall_seconds,
        output_hash,
        selftest: None,
        cpu_unknown: None,
        ended,
    }
}

/// Run `cmd` (program, args, env and stdio already set) as its own process group leader and
/// wait for it. Ends it at `job.wall_timeout` or when the watch says stop, kills the group,
/// then reaps it. Returns how it ended and the CPU seconds it used.
pub fn supervise(mut cmd: Command, job: &RowJob, start: Instant, watch: &dyn Watch) -> Supervised {
    let prog = cmd.get_program().to_string_lossy().into_owned();
    let child = cmd.process_group(0).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return Supervised::failed(format!("cannot start {prog}: {e}")),
    };
    let pid = child.id();
    watch.started(pid);
    // Set when the box ends the row itself: a stop or the wall timeout.
    let mut ended_by_box: Option<Exit> = None;
    let mut fault: Option<String> = None;
    // Detect exit without reaping: the zombie leader keeps the process group id,
    // so the group kill below cannot hit an unrelated group.
    loop {
        match sys::has_exited(pid) {
            Ok(true) => break,
            Ok(false) if !watch.tick() => {
                ended_by_box = Some(Exit::Stopped);
                break;
            }
            Ok(false) if start.elapsed() < job.wall_timeout => std::thread::sleep(POLL),
            Ok(false) => {
                // One more look: the row may have finished just now.
                if !matches!(sys::has_exited(pid), Ok(true)) {
                    ended_by_box = Some(Exit::Payload(TIMEOUT_CODE));
                }
                break;
            }
            Err(e) => {
                fault = Some(format!("wait failed: {e}"));
                break;
            }
        }
    }
    if let Err(e) = sys::kill_group(pid) {
        fault.get_or_insert(format!("kill failed: {e}"));
        // Kill the leader alone so the reap below cannot hang.
        let _ = child.kill();
    }
    let reaped = match sys::wait4(pid, true) {
        Ok(Some(r)) => r,
        Ok(None) => return Supervised::failed("wait returned early".into()),
        Err(e) => return Supervised::failed(format!("wait failed: {e}")),
    };
    let ended = ended_by_box.is_some();
    if let Some(f) = fault {
        return Supervised {
            exit: Exit::Infra(f),
            cpu_seconds: reaped.cpu_seconds,
            ended,
        };
    }
    let exit = ended_by_box.unwrap_or(match reaped.ended {
        Ended::Code(0) => Exit::Ok,
        Ended::Code(c) => Exit::Payload(c),
        Ended::Signal(s) => Exit::Payload(128 + s),
    });
    Supervised {
        exit,
        cpu_seconds: reaped.cpu_seconds,
        ended,
    }
}
