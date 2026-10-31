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
/// The lowest level that runs rows in the environment `pins.environment` names; level 0 runs
/// them on the host root.
pub const PINNED_LEVEL: u8 = 1;
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
    /// The payload's data limit in MiB; `None` leaves the box's own.
    pub memory_mib: Option<u32>,
}

impl RowJob {
    /// `memory_mib` in bytes, as every level applies it.
    pub fn data_limit(&self) -> Option<u64> {
        self.memory_mib.map(|m| u64::from(m) << 20)
    }
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
    /// What the sandbox's self-test found before the row; `None` at level 0 and when it
    /// reported nothing readable.
    pub selftest: Option<selftest::SelfTest>,
    /// Why the CPU count is missing the sandbox's own processes, when it is.
    pub cpu_unknown: Option<String>,
    /// The box ended the row: the watch said stop, or the wall timeout passed. The CPU
    /// count then misses what the killed processes used.
    pub ended: bool,
    /// The rows of the output links that `output_hash` covers and the output folder lacks:
    /// level 2 records each link its guest packed and makes none. Empty at levels 0 and 1,
    /// whose output folders hold their links.
    pub output_links: Vec<output::TreeRow>,
    /// CPU the box spent on the row outside `cpu_seconds`: level 2's VMM, its boot included.
    /// The host meter and the records count it as the box's spend. Zero at levels 0 and 1.
    pub box_cpu_seconds: f64,
}

impl RowOutcome {
    /// A row that ended with `exit` after `wall_seconds` with no CPU count, output or
    /// self-test.
    pub fn failed(exit: Exit, wall_seconds: f64) -> RowOutcome {
        RowOutcome {
            exit,
            cpu_seconds: 0.0,
            wall_seconds,
            output_hash: String::new(),
            selftest: None,
            cpu_unknown: None,
            ended: false,
            output_links: Vec::new(),
            box_cpu_seconds: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    Available,
    Unavailable(String),
}

/// What a backend tells the box while a row runs: the guest crate's watch, so level 2 hands
/// the runner's watch to `host::run_row`. `tick` runs over and over while the row runs.
/// `started` runs once for every supervised process group, in order: a level 1 row starts
/// its self-test group and then its own. Each call replaces the last; the earlier group is
/// already killed and reaped. Level 2 keeps `reaped` for itself, to fill
/// `RowOutcome::box_cpu_seconds`.
pub use ignorarium_guest::host::Watch;

pub trait Backend {
    fn level(&self) -> u8;
    fn probe(&self) -> Probe;
    /// What starts the payload, recorded beside the isolation level.
    fn launcher(&self) -> String;
    /// The hex SHA-256 of the program that sets up the payload inside the sandbox, recorded
    /// beside the launcher: level 2's guest agent. `None` for a backend without one, or while
    /// it cannot be read.
    fn agent_sha256(&self) -> Option<String> {
        None
    }
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
        output_links: Vec::new(),
        box_cpu_seconds: 0.0,
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
    let reaped = match sys::wait4(pid) {
        Ok(r) => r,
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
