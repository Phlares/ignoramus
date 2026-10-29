//! Level 0: a bare process on the host. No isolation.
use super::{Backend, Exit, Probe, RowJob, RowOutcome};
use crate::canon::{sha256_hex, to_canonical};
use crate::sys::{self, Ended};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Exit code reported for a row killed at its wall timeout.
pub const TIMEOUT_CODE: i32 = 124;
const POLL: Duration = Duration::from_millis(5);

pub struct Level0;

impl Backend for Level0 {
    fn level(&self) -> u8 {
        0
    }

    fn probe(&self) -> Probe {
        Probe::Available
    }

    fn run_row(&self, job: &RowJob) -> io::Result<RowOutcome> {
        let start = Instant::now();
        let (exit, cpu_seconds) = run_child(job, start);
        Ok(RowOutcome {
            exit,
            cpu_seconds,
            wall_seconds: start.elapsed().as_secs_f64(),
            output_hash: output_hash(&job.output_dir)?,
            selftest_ok: true,
        })
    }
}

fn run_child(job: &RowJob, start: Instant) -> (Exit, f64) {
    let Some((prog, args)) = job.argv.split_first() else {
        return (Exit::Infra("empty argv".into()), 0.0);
    };
    let child = Command::new(prog)
        .args(args)
        .env_clear()
        .envs(job.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .process_group(0)
        .spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => return (Exit::Infra(format!("cannot start {prog}: {e}")), 0.0),
    };
    let pid = child.id();
    let mut timed_out = false;
    let reaped = loop {
        match sys::wait4(pid, timed_out) {
            Ok(Some(r)) => break r,
            Ok(None) => {}
            Err(e) => return (Exit::Infra(format!("wait failed: {e}")), 0.0),
        }
        if start.elapsed() >= job.wall_timeout {
            timed_out = true;
            if let Err(e) = sys::kill_group(pid) {
                return (Exit::Infra(format!("kill failed: {e}")), 0.0);
            }
        } else {
            std::thread::sleep(POLL);
        }
    };
    let exit = if timed_out {
        Exit::Payload(TIMEOUT_CODE)
    } else {
        match reaped.ended {
            Ended::Code(0) => Exit::Ok,
            Ended::Code(c) => Exit::Payload(c),
            Ended::Signal(s) => Exit::Payload(128 + s),
        }
    };
    (exit, reaped.cpu_seconds)
}

/// Hash of the sorted `[relpath, kind, digest]` rows under `dir`.
/// Links are recorded, never followed.
pub fn output_hash(dir: &Path) -> io::Result<String> {
    let mut rows: Vec<(String, &'static str, String)> = Vec::new();
    walk(dir, "", &mut rows)?;
    rows.sort();
    let v = Value::Array(rows.into_iter().map(|(p, k, d)| json!([p, k, d])).collect());
    let bytes = to_canonical(&v).map_err(io::Error::other)?;
    Ok(sha256_hex(&bytes))
}

fn walk(
    dir: &Path,
    prefix: &str,
    rows: &mut Vec<(String, &'static str, String)>,
) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let rel = format!("{prefix}{}", entry.file_name().to_string_lossy());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk(&entry.path(), &format!("{rel}/"), rows)?;
        } else if ft.is_symlink() {
            rows.push((rel, "link", String::new()));
        } else if ft.is_file() {
            let mut f = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(entry.path())?;
            let mut bytes = Vec::new();
            f.read_to_end(&mut bytes)?;
            rows.push((rel, "file", sha256_hex(&bytes)));
        } else {
            rows.push((rel, "other", String::new()));
        }
    }
    Ok(())
}
