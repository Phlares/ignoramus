//! Level 0: a bare process on the host. No isolation.
use super::{Backend, Exit, Probe, RowJob, RowOutcome, Watch, outcome, supervise};
use crate::canon::{sha256_hex, to_canonical};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

pub struct Level0;

impl Backend for Level0 {
    fn level(&self) -> u8 {
        0
    }

    fn probe(&self) -> Probe {
        Probe::Available
    }

    fn launcher(&self) -> String {
        "process".into()
    }

    fn run_row(&self, job: &RowJob, watch: &dyn Watch) -> io::Result<RowOutcome> {
        let start = Instant::now();
        let (exit, cpu_seconds) = run_child(job, start, watch);
        Ok(outcome(job, start, exit, cpu_seconds))
    }
}

fn run_child(job: &RowJob, start: Instant, watch: &dyn Watch) -> (Exit, f64) {
    let Some((prog, args)) = job.argv.split_first() else {
        return (Exit::Infra("empty argv".into()), 0.0);
    };
    let mut cmd = Command::new(prog);
    cmd.args(args)
        .env_clear()
        .envs(job.env.iter().map(|(k, v)| (k, v)))
        .env("IGNORARIUM_IN", &job.input_dir)
        .env("IGNORARIUM_OUT", &job.output_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    supervise(cmd, job, start, watch)
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
