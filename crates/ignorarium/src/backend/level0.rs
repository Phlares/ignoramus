//! Level 0: a bare process on the host. No isolation.
use super::{Backend, Exit, Probe, RowJob, RowOutcome, Watch};
use crate::canon::{sha256_hex, to_canonical};
use crate::sys::{self, Ended};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
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

    fn run_row(&self, job: &RowJob, watch: &dyn Watch) -> io::Result<RowOutcome> {
        let start = Instant::now();
        let (mut exit, cpu_seconds) = run_child(job, start, watch);
        let wall_seconds = start.elapsed().as_secs_f64();
        // The payload ran: an output it left unhashable is its own result, not a box fault.
        let output_hash = match output_hash(&job.output_dir) {
            Ok(h) => h,
            Err(e) => {
                if exit == Exit::Ok {
                    exit = Exit::Void(format!("cannot hash output: {e}"));
                }
                String::new()
            }
        };
        Ok(RowOutcome {
            exit,
            cpu_seconds,
            wall_seconds,
            output_hash,
            selftest_ok: true,
        })
    }
}

fn run_child(job: &RowJob, start: Instant, watch: &dyn Watch) -> (Exit, f64) {
    let Some((prog, args)) = job.argv.split_first() else {
        return (Exit::Infra("empty argv".into()), 0.0);
    };
    let child = Command::new(prog)
        .args(args)
        .env_clear()
        .envs(job.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return (Exit::Infra(format!("cannot start {prog}: {e}")), 0.0),
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
        Ok(None) => return (Exit::Infra("wait returned early".into()), 0.0),
        Err(e) => return (Exit::Infra(format!("wait failed: {e}")), 0.0),
    };
    if let Some(f) = fault {
        return (Exit::Infra(f), reaped.cpu_seconds);
    }
    let exit = ended_by_box.unwrap_or(match reaped.ended {
        Ended::Code(0) => Exit::Ok,
        Ended::Code(c) => Exit::Payload(c),
        Ended::Signal(s) => Exit::Payload(128 + s),
    });
    (exit, reaped.cpu_seconds)
}

/// Hash of the sorted `[relpath, kind, digest]` rows under `dir`.
/// Links are recorded, never followed. A symlinked `dir` is refused.
pub fn output_hash(dir: &Path) -> io::Result<String> {
    if fs::symlink_metadata(dir)?.file_type().is_symlink() {
        return Err(io::Error::other("output dir is a symbolic link"));
    }
    let mut rows: Vec<(String, &'static str, String)> = Vec::new();
    walk(dir, "", &mut rows)?;
    rows.sort();
    let v = Value::Array(rows.into_iter().map(|(p, k, d)| json!([p, k, d])).collect());
    let bytes = to_canonical(&v).map_err(io::Error::other)?;
    Ok(sha256_hex(&bytes))
}

/// A name as text; bytes that are not UTF-8 become `!bytes:<hex>`.
fn name_of(n: &OsStr) -> String {
    match n.to_str() {
        Some(s) => s.to_string(),
        None => format!("!bytes:{}", hex::encode(n.as_bytes())),
    }
}

fn walk(
    dir: &Path,
    prefix: &str,
    rows: &mut Vec<(String, &'static str, String)>,
) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let rel = format!("{prefix}{}", name_of(&entry.file_name()));
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk(&entry.path(), &format!("{rel}/"), rows)?;
        } else if ft.is_symlink() {
            rows.push((rel, "link", String::new()));
        } else if ft.is_file() {
            rows.push(match file_digest(&entry.path())? {
                Some(d) => (rel, "file", d),
                None => (rel, "other", String::new()),
            });
        } else {
            rows.push((rel, "other", String::new()));
        }
    }
    Ok(())
}

/// SHA-256 of a regular file, streamed. `None` when the path is no longer a
/// regular file by the time it is open (swapped for a FIFO or a link).
fn file_digest(path: &Path) -> io::Result<Option<String>> {
    let opened = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path);
    let mut f = match opened {
        Ok(f) => f,
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return Ok(None),
        Err(e) => return Err(e),
    };
    if !f.metadata()?.is_file() {
        return Ok(None);
    }
    let mut h = Sha256::new();
    io::copy(&mut f, &mut h)?;
    Ok(Some(hex::encode(h.finalize())))
}
