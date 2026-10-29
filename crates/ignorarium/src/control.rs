//! Run control: pause, resume and stop requests, the run lock, the run state, the row in
//! flight and the deviation log. The files live in `<study>/run/`, except
//! `deviations.jsonl` in the study folder. A stop is final: no later request replaces it,
//! and a study with a stop record runs no further row.
use crate::canon::canonical_of;
use crate::events::{self, Event};
use crate::files::{append_line, open_log, replace_atomic};
use crate::home::Home;
use crate::records::{self, Record, RecordDraft, RecordError, VerifyError};
use crate::time::now_rfc3339;
use crate::{spec, sys};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The earnable label after a stop: the exploratory floor.
pub const LABEL_AFTER_STOP: &str = "signal";

/// A request to a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Signal {
    Pause,
    Stop(String),
    Resume,
}

impl Signal {
    /// What stands once `next` arrives after `held`: the latest request, but nothing
    /// replaces a stop.
    pub fn then(held: Option<Signal>, next: Signal) -> Signal {
        match held {
            Some(stop @ Signal::Stop(_)) => stop,
            _ => next,
        }
    }
}

/// The pending request of a study, one JSON line at `run/control`.
pub struct ControlFile {
    pub path: PathBuf,
}

impl ControlFile {
    pub fn of(home: &Home, study_id: &str) -> ControlFile {
        ControlFile {
            path: home.study_dir(study_id).join("run").join("control"),
        }
    }

    pub fn request_pause(&self) -> io::Result<()> {
        self.send(Signal::Pause)
    }

    pub fn request_resume(&self) -> io::Result<()> {
        self.send(Signal::Resume)
    }

    pub fn request_stop(&self, reason: &str) -> io::Result<()> {
        self.send(Signal::Stop(reason.into()))
    }

    /// The pending request. A pause or a resume is cleared; a stop stays, so a crash
    /// before its trace is written cannot lose it.
    pub fn take(&self) -> io::Result<Option<Signal>> {
        self.swap(true, |s| s.filter(|s| matches!(s, Signal::Stop(_))))
    }

    /// The pending request, left in place.
    pub fn peek(&self) -> io::Result<Option<Signal>> {
        self.swap(true, |s| s)
    }

    /// `peek` that never waits: while another handle holds the file's lock, it sees no
    /// request. The poll loop of a running row calls it.
    pub fn peek_now(&self) -> io::Result<Option<Signal>> {
        self.swap(false, |s| s)
    }

    fn send(&self, next: Signal) -> io::Result<()> {
        self.swap(true, |held| Some(Signal::then(held, next.clone())))
            .map(drop)
    }

    /// Replace the pending request with `f(pending)` under an exclusive lock; return the
    /// old one. Reading an absent file makes nothing. Bytes that do not parse read as a
    /// pause, the safe default. With `wait` false, a lock held elsewhere reads as no request.
    fn swap(
        &self,
        wait: bool,
        f: impl Fn(Option<Signal>) -> Option<Signal>,
    ) -> io::Result<Option<Signal>> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        let present = real_dir(dir, false)? && exists(&self.path)?;
        if !present && f(None).is_none() {
            return Ok(None);
        }
        real_dir(dir, true)?;
        let mut file = open_log(&self.path)?;
        if wait {
            file.lock()?;
        } else {
            match file.try_lock() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => return Ok(None),
                Err(TryLockError::Error(e)) => return Err(e),
            }
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let old = (!bytes.trim_ascii().is_empty())
            .then(|| serde_json::from_slice(&bytes).unwrap_or(Signal::Pause));
        let new = f(old.clone());
        if new != old {
            file.set_len(0)?;
            if let Some(s) = &new {
                let mut line = canonical_of(s)?;
                line.push(b'\n');
                file.write_all(&line)?;
            }
            file.sync_all()?;
        }
        Ok(old)
    }
}

/// Whether anything, a link included, sits at `path`.
fn exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Whether `dir` is a real folder, making it first when `make`. Its parent must exist, so
/// no study folder appears by accident. Anything else there, such as a link, is refused.
fn real_dir(dir: &Path, make: bool) -> io::Result<bool> {
    if make {
        let mut b = fs::DirBuilder::new();
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        match b.create(dir) {
            Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
            _ => {}
        }
    }
    match fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => Ok(true),
        Ok(_) => Err(io::Error::other(format!(
            "{} is not a folder",
            dir.display()
        ))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// `<study>/run/<name>`. With `make`, `run` is made when absent; reads make nothing.
fn run_path(home: &Home, study_id: &str, name: &str, make: bool) -> io::Result<PathBuf> {
    let run = home.study_dir(study_id).join("run");
    real_dir(&run, make)?;
    Ok(run.join(name))
}

/// The exclusive lock a run holds on `run/lock` for its whole length; dropping it unlocks.
/// Only its holder writes the study's stop trace.
pub struct RunLock {
    _file: File,
    home: Home,
    study_id: String,
}

/// How long a refused lock is retried. In a process with threads, a child between fork and
/// exec shares every open file, this lock included, for well under a millisecond.
const LOCK_GRACE: Duration = Duration::from_millis(50);

impl RunLock {
    /// The lock, or `None` while another holder has it. Never waits for a live run.
    pub fn try_take(home: &Home, study_id: &str) -> io::Result<Option<RunLock>> {
        let file = open_log(&run_path(home, study_id, "lock", true)?)?;
        let start = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => {
                    return Ok(Some(RunLock {
                        _file: file,
                        home: home.clone(),
                        study_id: study_id.into(),
                    }));
                }
                Err(TryLockError::WouldBlock) if start.elapsed() < LOCK_GRACE => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(TryLockError::WouldBlock) => return Ok(None),
                Err(TryLockError::Error(e)) => return Err(e),
            }
        }
    }

    /// Write the locked study's stop trace: a signed `stop` record, `State::Stopped`, one
    /// deviation row and a `run.stop` event. `in_flight` names a row the caller's run
    /// killed; a row a dead box left running is killed here while its leader lives. Each
    /// piece is written once: a second call adds nothing, and a later call writes what an
    /// earlier one could not. Returns the study's stop record.
    pub fn stop(
        &self,
        key: &SigningKey,
        reason: &str,
        in_flight: Option<u64>,
    ) -> Result<Record, ControlError> {
        let (home, id) = (&self.home, self.study_id.as_str());
        let dir = home.study_dir(id);
        let log = dir.join("records.jsonl");
        let all = records::read_all(&log, &key.verifying_key())?;
        let record = match all.iter().find(|r| r.kind == "stop") {
            Some(r) => r.clone(),
            None => {
                let (left, note) = match end_orphan(home, id) {
                    Ok(Some(o)) => (Some(o.row), Some(o.note)),
                    Ok(None) => (None, None),
                    Err(e) => (
                        None,
                        Some(format!(
                            "cannot end the row a dead box left: {e}; group not killed"
                        )),
                    ),
                };
                let rows_done = all
                    .iter()
                    .filter(|r| r.kind == "row")
                    .filter_map(|r| r.row)
                    .collect::<BTreeSet<_>>()
                    .len();
                let body = json!({
                    "who": who(),
                    "when": now_rfc3339(),
                    "why": reason,
                    "label_before": spec_label(&dir),
                    "label_after": LABEL_AFTER_STOP,
                    "rows_done": rows_done,
                    "in_flight": in_flight.or(left),
                    "note": note,
                });
                let draft = RecordDraft {
                    study: id.into(),
                    row: None,
                    kind: "stop".into(),
                    body,
                };
                records::append(&log, key, draft)?
            }
        };
        let why = record.body["why"].as_str().unwrap_or(reason);
        State::Stopped(why.into()).write(home, id)?;
        if !logged(&dir.join("deviations.jsonl"), "kind", "stop")? {
            deviation(home, id, &stop_deviation(&dir, &record.body)?)?;
        }
        if !logged(&dir.join("events.jsonl"), "name", "run.stop")? {
            let hash = all
                .iter()
                .find(|r| r.kind == "register")
                .and_then(|r| r.body["spec_hash"].as_str())
                .and_then(spec::hash_bytes)
                .ok_or_else(|| {
                    ControlError::Records("no register record names a spec hash".into())
                })?;
            let event = Event {
                ts: now_rfc3339(),
                trace_id: events::trace_id(&hash),
                span_id: None,
                name: "run.stop".into(),
                level: "warn".into(),
                attrs: json!({ "why": why, "rows_done": record.body["rows_done"] }),
            };
            events::emit(&dir.join("events.jsonl"), event)?;
        }
        Ok(record)
    }
}

/// Write the stop trace here when a stop is pending and no run holds the lock. `None` when
/// no stop is pending, or when a live run holds the lock: it sees the stop at its next tick.
pub fn stop_if_idle(
    home: &Home,
    study_id: &str,
    key: &SigningKey,
) -> Result<Option<Record>, ControlError> {
    let Some(Signal::Stop(reason)) = ControlFile::of(home, study_id).peek()? else {
        return Ok(None);
    };
    let Some(lock) = RunLock::try_take(home, study_id)? else {
        return Ok(None);
    };
    lock.stop(key, &reason, None).map(Some)
}

/// Whether a run holds the study's lock now. A shared probe, released at once: it never
/// takes the exclusive lock, so it neither blocks a starting run nor reads as a holder
/// to another probe. A study that never ran has no lock.
pub fn run_is_live(home: &Home, study_id: &str) -> io::Result<bool> {
    let lock = run_path(home, study_id, "lock", false)?;
    let file = match File::open(&lock) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    match file.try_lock_shared() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

/// How often `await_stop` looks again.
const STOP_POLL: Duration = Duration::from_millis(50);

/// Wait up to `wait` for the study's stop record: write the trace here when no run holds
/// the lock, else let the live run write it. `None` when the wait runs out.
pub fn await_stop(
    home: &Home,
    study_id: &str,
    key: &SigningKey,
    wait: Duration,
) -> Result<Option<Record>, ControlError> {
    let log = home.study_dir(study_id).join("records.jsonl");
    let start = Instant::now();
    loop {
        if let Some(r) = stop_if_idle(home, study_id, key)? {
            return Ok(Some(r));
        }
        let all = records::read_all(&log, &key.verifying_key())?;
        if let Some(r) = all.into_iter().find(|r| r.kind == "stop") {
            return Ok(Some(r));
        }
        if start.elapsed() >= wait {
            return Ok(None);
        }
        std::thread::sleep(STOP_POLL);
    }
}

/// The last state a run wrote, at `run/state.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Running,
    Paused(String),
    Stopped(String),
    Done,
}

impl State {
    /// `None` before the first run.
    pub fn read(home: &Home, study_id: &str) -> io::Result<Option<State>> {
        match fs::read(run_path(home, study_id, "state.json", false)?) {
            Ok(b) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(io::Error::other),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn write(&self, home: &Home, study_id: &str) -> io::Result<()> {
        let mut bytes = canonical_of(self)?;
        bytes.push(b'\n');
        replace_atomic(
            &run_path(home, study_id, "state.json", true)?,
            &bytes,
            0o600,
        )
    }
}

/// The row in flight, kept at `run/row.json` while its process group runs, so a stop can
/// still find the group after the box dies.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RowFile {
    row: u64,
    span: String,
    pid: u32,
    pgid: u32,
    start: u64,
}

/// Row `row` (span `span`) now runs as process `pid`.
pub fn row_started(home: &Home, study_id: &str, row: u64, span: &str, pid: u32) -> io::Result<()> {
    let (pgid, start) = proc_stat(pid)?;
    let file = RowFile {
        row,
        span: span.into(),
        pid,
        pgid,
        start,
    };
    let mut bytes = canonical_of(&file)?;
    bytes.push(b'\n');
    replace_atomic(&run_path(home, study_id, "row.json", true)?, &bytes, 0o600)
}

/// The row's process group is gone. A file left behind is harmless: its leader is reaped,
/// so the start-time check refuses to kill on its say.
pub fn row_ended(home: &Home, study_id: &str) {
    let _ = fs::remove_file(home.study_dir(study_id).join("run").join("row.json"));
}

/// A row a dead box left running.
pub struct Orphan {
    pub row: u64,
    pub span: String,
    pub note: String,
}

/// Kill the process group of a row a dead box left running. The group is killed only while
/// its leader lives with the recorded start time and still leads it: a live leader keeps
/// the group id from reuse. `Ok(None)` when no row was left; a file that cannot be read
/// stays in place.
pub fn end_orphan(home: &Home, study_id: &str) -> io::Result<Option<Orphan>> {
    let bytes = match fs::read(run_path(home, study_id, "row.json", false)?) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let left: RowFile = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let leads = |(pgid, start): (u32, u64)| start == left.start && pgid == left.pgid && pgid > 1;
    let note = if proc_stat(left.pid).is_ok_and(leads) {
        sys::kill_group(left.pgid)?;
        "row leader alive; group killed"
    } else {
        "row leader gone; group not killed"
    };
    row_ended(home, study_id);
    Ok(Some(Orphan {
        row: left.row,
        span: left.span,
        note: note.into(),
    }))
}

/// `(process group id, start time)` of process `pid`, from `/proc/<pid>/stat`.
fn proc_stat(pid: u32) -> io::Result<(u32, u64)> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    stat_fields(&text)
        .ok_or_else(|| io::Error::other(format!("/proc/{pid}/stat names no group or start")))
}

/// Fields 5 (process group) and 22 (start time) of a stat line. The command name, field 2,
/// sits in parentheses and may hold spaces and parentheses, so fields count from the last `)`.
fn stat_fields(text: &str) -> Option<(u32, u64)> {
    let (_, rest) = text.rsplit_once(')')?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // `rest` starts at field 3, so field n is fields[n - 3].
    Some((fields.get(2)?.parse().ok()?, fields.get(19)?.parse().ok()?))
}

/// When a deviation happened: five stages, after Willroth and Atherton (2024) on
/// reporting preregistration deviations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Timing {
    BeforeCollection,
    DuringCollection,
    AfterCollectionBeforeAccess,
    AfterAccessBeforeResults,
    AfterResults,
}

impl Timing {
    /// A stop with `done` rows recorded and a row killed `in_flight` or not. `total`, the
    /// scheduled rows, is read only when it decides. The box reads no result before the
    /// report, so a stop after the last row is still before access.
    pub fn of_stop(
        done: usize,
        in_flight: bool,
        total: impl FnOnce() -> io::Result<usize>,
    ) -> io::Result<Timing> {
        Ok(if in_flight || (done > 0 && done < total()?) {
            Timing::DuringCollection
        } else if done == 0 {
            Timing::BeforeCollection
        } else {
            Timing::AfterCollectionBeforeAccess
        })
    }
}

/// One row of the deviation log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviationRow {
    pub kind: String,
    pub reason: String,
    pub timing: Timing,
    pub original: String,
    pub justification: String,
    pub impact: String,
}

/// Append one row to the study's `deviations.jsonl`.
pub fn deviation(home: &Home, study_id: &str, row: &DeviationRow) -> io::Result<()> {
    append_line(
        &home.study_dir(study_id).join("deviations.jsonl"),
        canonical_of(row)?,
    )
}

#[derive(Debug)]
pub enum ControlError {
    Io(io::Error),
    /// The record log does not verify or cannot take a record.
    Records(String),
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ControlError::Io(e) => write!(f, "I/O failed: {e}"),
            ControlError::Records(e) => write!(f, "records: {e}"),
        }
    }
}

impl std::error::Error for ControlError {}

impl From<io::Error> for ControlError {
    fn from(e: io::Error) -> Self {
        ControlError::Io(e)
    }
}

impl From<RecordError> for ControlError {
    fn from(e: RecordError) -> Self {
        ControlError::Records(e.to_string())
    }
}

impl From<VerifyError> for ControlError {
    fn from(e: VerifyError) -> Self {
        ControlError::Records(e.to_string())
    }
}

/// The deviation row for a stop, from its signed record.
fn stop_deviation(dir: &Path, body: &Value) -> io::Result<DeviationRow> {
    let text = |k: &str| body[k].as_str().unwrap_or_default();
    let done = body["rows_done"].as_u64().unwrap_or(0);
    let timing = Timing::of_stop(
        usize::try_from(done).unwrap_or(usize::MAX),
        !body["in_flight"].is_null(),
        || scheduled_rows(dir),
    )?;
    Ok(DeviationRow {
        kind: "stop".into(),
        reason: text("why").into(),
        timing,
        original: "run every row of the schedule".into(),
        justification: format!("break-glass stop by {}", text("who")),
        impact: format!(
            "{done} rows ran before the stop; the earnable label drops from {} to {}",
            text("label_before"),
            text("label_after")
        ),
    })
}

/// True when a JSON line of the log at `path` has `key` equal to `value`.
fn logged(path: &Path, key: &str, value: &str) -> io::Result<bool> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    Ok(text
        .lines()
        .any(|l| serde_json::from_str::<Value>(l).is_ok_and(|v| v[key] == value)))
}

/// Rows in the written schedule. A missing or broken schedule is an error.
fn scheduled_rows(dir: &Path) -> io::Result<usize> {
    let fail = |e: &dyn fmt::Display| {
        io::Error::other(format!("cannot count the rows of schedule.json: {e}"))
    };
    let bytes = fs::read(dir.join("schedule.json")).map_err(|e| fail(&e))?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| fail(&e))?;
    v["rows"]
        .as_array()
        .map(Vec::len)
        .ok_or_else(|| fail(&"no rows array"))
}

/// `study.label` of the stored spec, or `unknown`: a stop never fails on an unreadable spec.
fn spec_label(dir: &Path) -> String {
    spec::load(&dir.join("spec.toml"))
        .ok()
        .and_then(|s| s.value["study"]["label"].as_str().map(String::from))
        .unwrap_or_else(|| "unknown".into())
}

/// The OS user running this process: its name in `/etc/passwd`, else its uid number.
pub fn who() -> String {
    let passwd = fs::read_to_string("/etc/passwd").unwrap_or_default();
    user_of(&passwd, rustix::process::getuid().as_raw())
}

/// The name on the first passwd line whose uid field is `uid`, else `uid` as a number.
pub fn user_of(passwd: &str, uid: u32) -> String {
    let uid = uid.to_string();
    passwd
        .lines()
        .find_map(|l| {
            let mut fields = l.split(':');
            let name = fields.next()?;
            (fields.nth(1)? == uid).then(|| name.to_string())
        })
        .unwrap_or(uid)
}

#[cfg(test)]
mod tests {
    use super::stat_fields;

    #[test]
    fn stat_fields_count_from_the_last_parenthesis() {
        let line =
            "4242 (a) (b c) S 1 4243 4243 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1234 56\n";
        assert_eq!(stat_fields(line), Some((4243, 987654)));
        assert_eq!(stat_fields("4242 (a) S 1 4243"), None);
        assert_eq!(stat_fields("no parenthesis"), None);
    }
}
