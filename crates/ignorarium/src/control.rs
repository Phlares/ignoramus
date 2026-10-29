//! Run control: pause, resume and stop requests, the run lock, the run state and the
//! deviation log. The files live in `<study>/run/`, except `deviations.jsonl` in the study
//! folder. A stop is final: no later request replaces it, and a study with a stop record runs
//! no further row.
use crate::canon::canonical_of;
use crate::events::{self, Event};
use crate::files::{append_line, open_log, replace_atomic};
use crate::home::Home;
use crate::records::{self, Record, RecordDraft, RecordError, VerifyError};
use crate::time::now_rfc3339;
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
        self.swap(|s| s.filter(|s| matches!(s, Signal::Stop(_))))
    }

    /// The pending request, left in place.
    pub fn peek(&self) -> io::Result<Option<Signal>> {
        self.swap(|s| s)
    }

    fn send(&self, next: Signal) -> io::Result<()> {
        self.swap(|held| Some(Signal::then(held, next))).map(drop)
    }

    /// Replace the pending request with `f(pending)` under an exclusive lock; return the
    /// old one. Bytes that do not parse read as a pause, the safe default.
    fn swap(&self, f: impl FnOnce(Option<Signal>) -> Option<Signal>) -> io::Result<Option<Signal>> {
        if let Some(dir) = self.path.parent() {
            ensure_dir(dir)?;
        }
        let mut file = open_log(&self.path)?;
        file.lock()?;
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

/// Make `dir` in its existing parent: a missing study folder is an error, never created.
/// Anything there but a real folder, such as a symbolic link, is refused.
fn ensure_dir(dir: &Path) -> io::Result<()> {
    let mut b = fs::DirBuilder::new();
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    match b.create(dir) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
        _ => {}
    }
    if fs::symlink_metadata(dir)?.is_dir() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{} is not a folder",
            dir.display()
        )))
    }
}

/// `<study>/run/<name>`, making `run` when it is absent.
fn run_path(home: &Home, study_id: &str, name: &str) -> io::Result<PathBuf> {
    let run = home.study_dir(study_id).join("run");
    ensure_dir(&run)?;
    Ok(run.join(name))
}

/// The exclusive lock a run holds on `run/lock` for its whole length. Dropping it unlocks.
pub struct RunLock {
    _file: File,
}

/// How long a refused lock is retried. In a process with threads, a child between fork and
/// exec shares every open file, this lock included, for well under a millisecond.
const LOCK_GRACE: Duration = Duration::from_millis(50);

impl RunLock {
    /// The lock, or `None` while another holder has it. Never waits for a live run.
    pub fn try_take(home: &Home, study_id: &str) -> io::Result<Option<RunLock>> {
        let file = open_log(&run_path(home, study_id, "lock")?)?;
        let start = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Some(RunLock { _file: file })),
                Err(TryLockError::WouldBlock) if start.elapsed() < LOCK_GRACE => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(TryLockError::WouldBlock) => return Ok(None),
                Err(TryLockError::Error(e)) => return Err(e),
            }
        }
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
        match fs::read(run_path(home, study_id, "state.json")?) {
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
        replace_atomic(&run_path(home, study_id, "state.json")?, &bytes, 0o600)
    }
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
    /// A stop with `done` of `total` rows recorded. The box reads no result before the
    /// report, so a stop after the last row is still before access.
    pub fn of_stop(done: usize, total: usize) -> Timing {
        match done {
            0 => Timing::BeforeCollection,
            d if d < total => Timing::DuringCollection,
            _ => Timing::AfterCollectionBeforeAccess,
        }
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

/// Write the stop trace: a signed `stop` record, one deviation row, a `run.stop` event and
/// `State::Stopped`. Holding the run lock makes the caller the only writer. Each piece is
/// written once: a second call adds nothing, and a call after a crash fills in what is
/// missing. Returns the study's stop record.
pub fn stop(
    home: &Home,
    study_id: &str,
    key: &SigningKey,
    reason: &str,
    _lock: &RunLock,
) -> Result<Record, ControlError> {
    let dir = home.study_dir(study_id);
    let log = dir.join("records.jsonl");
    let all = records::read_all(&log, &key.verifying_key())?;
    let record = match all.iter().find(|r| r.kind == "stop") {
        Some(r) => r.clone(),
        None => {
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
            });
            let draft = RecordDraft {
                study: study_id.into(),
                row: None,
                kind: "stop".into(),
                body,
            };
            let r = records::append(&log, key, draft)?;
            let event = Event {
                ts: now_rfc3339(),
                trace_id: trace_of(&all),
                span_id: None,
                name: "run.stop".into(),
                level: "warn".into(),
                attrs: json!({ "why": reason, "rows_done": rows_done }),
            };
            events::emit(&dir.join("events.jsonl"), event)?;
            r
        }
    };
    if !stop_logged(&dir)? {
        deviation(
            home,
            study_id,
            &stop_deviation(&record.body, scheduled_rows(&dir)),
        )?;
    }
    let why = record.body["why"].as_str().unwrap_or(reason);
    State::Stopped(why.into()).write(home, study_id)?;
    Ok(record)
}

/// Write the stop trace here when a stop is pending and no run holds the lock. `None` when
/// no stop is pending, or when a live run holds the lock: it sees the stop at its next tick.
pub fn stop_if_idle(
    home: &Home,
    study_id: &str,
    key: &SigningKey,
    control: &ControlFile,
) -> Result<Option<Record>, ControlError> {
    let Some(Signal::Stop(reason)) = control.peek()? else {
        return Ok(None);
    };
    let Some(lock) = RunLock::try_take(home, study_id)? else {
        return Ok(None);
    };
    stop(home, study_id, key, &reason, &lock).map(Some)
}

/// The deviation row for a stop, from its signed record.
fn stop_deviation(body: &Value, total: usize) -> DeviationRow {
    let text = |k: &str| body[k].as_str().unwrap_or_default();
    let done = body["rows_done"].as_u64().unwrap_or(0);
    DeviationRow {
        kind: "stop".into(),
        reason: text("why").into(),
        timing: Timing::of_stop(usize::try_from(done).unwrap_or(usize::MAX), total),
        original: "run every row of the schedule".into(),
        justification: format!("break-glass stop by {}", text("who")),
        impact: format!(
            "{done} rows ran before the stop; the earnable label drops from {} to {}",
            text("label_before"),
            text("label_after")
        ),
    }
}

/// True when the deviation log already holds the stop's row.
fn stop_logged(dir: &Path) -> io::Result<bool> {
    let text = match fs::read_to_string(dir.join("deviations.jsonl")) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    Ok(text
        .lines()
        .any(|l| serde_json::from_str::<Value>(l).is_ok_and(|v| v["kind"] == "stop")))
}

/// Rows in the written schedule; 0 before one is written.
fn scheduled_rows(dir: &Path) -> usize {
    fs::read(dir.join("schedule.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["rows"].as_array().map(Vec::len))
        .unwrap_or(0)
}

/// `study.label` of the stored spec, or `unknown`: a stop never fails on an unreadable spec.
fn spec_label(dir: &Path) -> String {
    crate::spec::load(&dir.join("spec.toml"))
        .ok()
        .and_then(|s| s.value["study"]["label"].as_str().map(String::from))
        .unwrap_or_else(|| "unknown".into())
}

/// The study's trace id: the first 16 bytes of the registered spec hash, in hex.
fn trace_of(all: &[Record]) -> String {
    all.iter()
        .find(|r| r.kind == "register")
        .and_then(|r| r.body["spec_hash"].as_str())
        .and_then(|h| h.strip_prefix("sha256:"))
        .and_then(|h| h.get(..32))
        .unwrap_or_default()
        .to_string()
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
