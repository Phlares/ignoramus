//! The run engine: writes the schedule, runs each row on a backend, records each row.
//! A row is done when its signed record exists. A crash before the record reruns the row.
use crate::backend::{self, Backend, Exit, Probe, RowJob, RowOutcome, Watch};
use crate::budget::{Alerts, Cost, CostMeter, Decision, HostMeter, decide, dimension_name};
use crate::canon::sha256_hex;
use crate::control::{self, ControlError, ControlFile, RunLock, Signal, State};
use crate::estimate::{apply_defaults, estimate, units};
use crate::events::{self, Event};
use crate::home::Home;
use crate::machine::{self, Machine};
use crate::planlog::store::{Leaf, LogError, PlanLog};
use crate::prior::file_hashes;
use crate::records::{self, RecordDraft, RecordError, VerifyError};
use crate::rules::input_bytes;
use crate::schedule::{self, Row, Schedule, ScheduleError};
use crate::spec::model::{Role, Spec};
use crate::{keys, spec};
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Infrastructure failures retried per row, after a health check each.
pub const MAX_RETRIES: u32 = 3;
/// Signs of life while a row runs: one `heartbeat` event per interval.
pub const HEARTBEAT: Duration = Duration::from_secs(15);
const MIN_TIMEOUT: Duration = Duration::from_secs(60);
const TIMEOUT_FACTOR: f64 = 10.0;
const PATH_ENV: &str = "/usr/bin:/bin";
const PAUSE_REQUESTED: &str = "a pause was requested";

pub struct RunPlan {
    pub home: Home,
    pub study_id: String,
    pub spec: Spec,
    pub spec_hash: [u8; 32],
    pub schedule: Schedule,
    pub backend: Box<dyn Backend>,
    pub key: SigningKey,
    /// `(exact, normalized)` hashes of the inputs present now.
    pub data_hashes: Vec<(String, String)>,
    pub payload: PathBuf,
    pub payload_sha256: String,
    /// Cumulative cost, read before each row.
    pub meter: Box<dyn CostMeter>,
    /// Interval between `heartbeat` events while a row runs.
    pub heartbeat: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    Done,
    Paused(String),
    Stopped(String),
}

#[derive(Debug)]
pub enum RunError {
    /// The study has no `register` record.
    NotRegistered(String),
    /// The stored spec no longer hashes to the registered hash.
    SpecChanged,
    /// The payload file differs from `pins.payload`.
    PayloadChanged,
    NoPayload(String),
    NoKey,
    /// Another run of this study holds the run lock.
    Busy,
    /// A path inside the study folder is a symbolic link.
    Unsafe(String),
    NoBackend {
        need: u8,
        have: Vec<u8>,
    },
    /// A selection or confirmation input that was absent at registration was used before.
    DataReused {
        path: String,
        label: String,
    },
    Spec(String),
    Schedule(ScheduleError),
    Log(LogError),
    Records(String),
    Io(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::NotRegistered(id) => write!(f, "study {id} has no register record"),
            RunError::SpecChanged => write!(f, "the stored spec differs from the registered hash"),
            RunError::PayloadChanged => write!(f, "the payload differs from pins.payload"),
            RunError::NoPayload(e) => write!(f, "cannot read the payload: {e}"),
            RunError::NoKey => write!(f, "the box key does not exist"),
            RunError::Busy => write!(f, "another run of this study is in progress"),
            RunError::Unsafe(p) => write!(f, "{p} is a symbolic link"),
            RunError::NoBackend { need, have } => {
                write!(
                    f,
                    "no backend at isolation level {need} or higher; have {have:?}"
                )
            }
            RunError::DataReused { path, label } => write!(
                f,
                "E-CNF-005: input {path} was used by an earlier study ({label}). See docs/rules/E-CNF-005.md"
            ),
            RunError::Spec(e) => write!(f, "cannot load the spec: {e}"),
            RunError::Schedule(e) => e.fmt(f),
            RunError::Log(e) => write!(f, "plan log: {e}"),
            RunError::Records(e) => write!(f, "records: {e}"),
            RunError::Io(e) => write!(f, "I/O failed: {e}"),
        }
    }
}

impl std::error::Error for RunError {}

impl From<LogError> for RunError {
    fn from(e: LogError) -> Self {
        RunError::Log(e)
    }
}

impl From<ScheduleError> for RunError {
    fn from(e: ScheduleError) -> Self {
        RunError::Schedule(e)
    }
}

impl From<VerifyError> for RunError {
    fn from(e: VerifyError) -> Self {
        RunError::Records(e.to_string())
    }
}

impl From<RecordError> for RunError {
    fn from(e: RecordError) -> Self {
        RunError::Records(e.to_string())
    }
}

impl From<ControlError> for RunError {
    fn from(e: ControlError) -> Self {
        match e {
            ControlError::Io(e) => RunError::Io(e.to_string()),
            ControlError::Records(e) => RunError::Records(e),
        }
    }
}

fn io(e: impl fmt::Display) -> RunError {
    RunError::Io(e.to_string())
}

/// `prepare_with` on the backends this build registers, up to `backend_max`.
pub fn prepare(home: &Home, study_id: &str, backend_max: u8) -> Result<RunPlan, RunError> {
    prepare_with(home, study_id, up_to(backend::registered(), backend_max))
}

/// The backends at or below `max`.
pub fn up_to(backends: Vec<Box<dyn Backend>>, max: u8) -> Vec<Box<dyn Backend>> {
    backends.into_iter().filter(|b| b.level() <= max).collect()
}

/// Load a registered study, gate its late data, write its schedule, and pick the lowest
/// backend that meets `scope.min_isolation`.
pub fn prepare_with(
    home: &Home,
    study_id: &str,
    backends: Vec<Box<dyn Backend>>,
) -> Result<RunPlan, RunError> {
    let dir = home.study_dir(study_id);
    let key = keys::load(home).map_err(io)?.ok_or(RunError::NoKey)?;
    let all = records::read_all(&dir.join("records.jsonl"), &key.verifying_key())?;
    let register = all
        .iter()
        .find(|r| r.kind == "register")
        .ok_or_else(|| RunError::NotRegistered(study_id.into()))?;
    let raw = spec::load(&dir.join("spec.toml")).map_err(|e| RunError::Spec(e.to_string()))?;
    if register.body["spec_hash"].as_str() != Some(raw.hash.as_str()) {
        return Err(RunError::SpecChanged);
    }
    let mut value = raw.value;
    apply_defaults(&mut value);
    let spec = Spec::from_value(&value).map_err(|e| RunError::Spec(e.to_string()))?;
    let spec_hash = decode_hash(&raw.hash)?;

    let have: Vec<u8> = backends.iter().map(|b| b.level()).collect();
    let need = spec.scope.min_isolation;
    let backend = backends
        .into_iter()
        .filter(|b| b.level() >= need)
        .min_by_key(|b| b.level())
        .ok_or(RunError::NoBackend { need, have })?;

    no_links(&dir, &["payload", "run"])?;
    let payload = std::path::absolute(dir.join("payload").join("run")).map_err(io)?;
    let payload_sha256 =
        sha256_hex(&std::fs::read(&payload).map_err(|e| RunError::NoPayload(e.to_string()))?);
    let pin = spec.pins.payload.trim();
    let pin = pin.strip_prefix("sha256:").unwrap_or(pin);
    if !pin.is_empty() && pin != payload_sha256 {
        return Err(RunError::PayloadChanged);
    }

    let leaf = register.body["leaf"]
        .as_u64()
        .ok_or_else(|| RunError::NotRegistered(study_id.into()))?;
    let log = match register.body["log"].as_str() {
        Some(l @ ("planlog" | "testlog")) => l,
        other => {
            return Err(RunError::Records(format!(
                "register record names log {other:?}"
            )));
        }
    };
    let machine = machine::load(&home.root).map_err(io)?;
    let prior = Tally::of(&all).cost(0.0);
    let data_hashes = gate_late_data(home, &spec, log, leaf, &key)?;
    let rows = schedule::expand(&spec, &spec_hash, &schedule::units_from_spec(&spec))
        .map_err(|e| RunError::Spec(e.to_string()))?;
    let schedule = Schedule::new(&spec_hash, rows);
    schedule::write_once(&dir, &schedule)?;
    Ok(RunPlan {
        home: home.clone(),
        study_id: study_id.into(),
        spec,
        spec_hash,
        schedule,
        backend,
        key,
        data_hashes,
        payload,
        payload_sha256,
        meter: Box::new(HostMeter::new(prior, machine.rate_usd_per_hour)),
        heartbeat: HEARTBEAT,
    })
}

fn decode_hash(h: &str) -> Result<[u8; 32], RunError> {
    h.strip_prefix("sha256:")
        .and_then(|x| hex::decode(x).ok())
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| RunError::Spec(format!("bad spec hash {h}")))
}

/// Hash every input. A selection or confirmation input that the registered plan leaf did not
/// hold is late: refuse if another study used it, else record it under this study's id once.
/// Every lookup finishes before the append takes its lock.
fn gate_late_data(
    home: &Home,
    spec: &Spec,
    log_name: &str,
    leaf_index: u64,
    key: &SigningKey,
) -> Result<Vec<(String, String)>, RunError> {
    let id = spec.study.id.as_str();
    let dir = home.study_dir(id);
    let log = PlanLog::open(&home.root.join(log_name))?;
    let registered = log
        .leaf(leaf_index)?
        .ok_or_else(|| RunError::NotRegistered(id.into()))?
        .data_hashes;
    let mut all = Vec::new();
    let mut late = Vec::new();
    for input in &spec.inputs {
        let Some(bytes) = input_bytes(&dir, &input.path) else {
            continue;
        };
        let hashes = file_hashes(&bytes);
        all.push(hashes.clone());
        if matches!(input.role, Role::Selection | Role::Confirmation)
            && !registered.contains(&hashes)
        {
            late.push((input.path.clone(), hashes));
        }
    }
    let mut new = Vec::new();
    for (path, hashes) in late {
        let uses = log.uses(&hashes.0, &hashes.1)?;
        if let Some((i, leaf)) = uses.iter().find(|(_, l)| !is_own(l, id)) {
            let label = match leaf.kind.as_str() {
                "legacy" => format!("study {}", leaf.label),
                _ => format!("registered plan, leaf {i}"),
            };
            return Err(RunError::DataReused { path, label });
        }
        if uses.is_empty() {
            new.push(hashes);
        }
    }
    if !new.is_empty() {
        log.append_legacy(id, new, key)?;
    }
    Ok(all)
}

/// A late-data leaf this study appended. A plan leaf is never its own use.
fn is_own(leaf: &Leaf, id: &str) -> bool {
    leaf.kind == "legacy" && leaf.label == id
}

/// Refuse a symbolic link at any step of `parts` below `base`. Steps that do not exist yet are fine.
fn no_links(base: &Path, parts: &[&str]) -> Result<(), RunError> {
    let mut at = base.to_path_buf();
    for p in parts {
        at.push(p);
        match at.symlink_metadata() {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(RunError::Unsafe(at.display().to_string()));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(io(e)),
        }
    }
    Ok(())
}

/// Run every row that has no record, holding the run lock; a second run gets `Busy`.
/// At each row boundary the control file is read: a stop ends the run with the stop trace,
/// a pause ends it at that boundary. Then the meter is read: the run pauses instead of
/// starting a row when a cap is reached or the row would reach one. While a row runs, the
/// backend ticks the watch, which emits heartbeats and ends the row on a stop.
pub fn run(plan: &RunPlan, control: &ControlFile) -> Result<RunStatus, RunError> {
    no_links(&plan.home.study_dir(&plan.study_id), &["run"])?;
    let status = {
        let lock = RunLock::try_take(&plan.home, &plan.study_id)
            .map_err(io)?
            .ok_or(RunError::Busy)?;
        run_locked(plan, control, &lock)?
    };
    // A stop sent after this run's last look found the lock still held: finish it here.
    Ok(
        match control::stop_if_idle(&plan.home, &plan.study_id, &plan.key, control)? {
            Some(r) => RunStatus::Stopped(why_of(&r)),
            None => status,
        },
    )
}

fn run_locked(
    plan: &RunPlan,
    control: &ControlFile,
    lock: &RunLock,
) -> Result<RunStatus, RunError> {
    let dir = plan.home.study_dir(&plan.study_id);
    let log = dir.join("records.jsonl");
    let machine = machine::load(&plan.home.root).map_err(io)?;
    let all = records::read_all(&log, &plan.key.verifying_key())?;
    if let Some(r) = all.iter().find(|r| r.kind == "stop") {
        return halt(plan, &dir, lock, Halt::Stop(why_of(r)));
    }
    State::Running
        .write(&plan.home, &plan.study_id)
        .map_err(io)?;
    let watch = RunWatch {
        plan,
        control,
        dir: &dir,
        heard: RefCell::new(None),
        row: RefCell::new((0, String::new())),
        last_beat: Cell::new(Instant::now()),
    };
    let done: BTreeSet<u64> = all
        .iter()
        .filter(|r| r.kind == "row")
        .filter_map(|r| r.row)
        .collect();
    let mut tally = Tally::of(&all);
    let per_row = row_estimate(plan, &machine);
    let budget = &plan.spec.scope.budget;
    // Only spend from earlier sessions stays silent; this session's first reading may alert.
    let mut alerts = Alerts::seeded(&tally.cost(machine.rate_usd_per_hour), budget);
    for row in plan
        .schedule
        .rows
        .iter()
        .filter(|r| !done.contains(&r.index))
    {
        if let Some(h) = watch.at_boundary() {
            return halt(plan, &dir, lock, h);
        }
        let sampled = plan.meter.sample();
        let now = sampled.clone().unwrap_or_default();
        if let Ok(c) = &sampled {
            for (dim, fraction) in alerts.fresh(c, budget) {
                let attrs = json!({ "dimension": dimension_name(dim), "fraction": fraction });
                log_event(plan, &dir, None, "budget.alert", "warn", attrs)?;
            }
        }
        let next = per_row.larger(&tally.mean(machine.rate_usd_per_hour));
        if let Decision::Pause(why) = decide(&now, &next, budget, sampled.map(|_| ())) {
            return halt(plan, &dir, lock, Halt::Pause(why.to_string()));
        }
        if let Some(h) = run_row(plan, row, &dir, &log, &mut tally, &watch)? {
            return halt(plan, &dir, lock, h);
        }
    }
    State::Done.write(&plan.home, &plan.study_id).map_err(io)?;
    Ok(RunStatus::Done)
}

/// Why a run ends before its last row.
enum Halt {
    Pause(String),
    Stop(String),
}

/// End the run. A pause writes `run.pause` and the paused state; a stop writes the stop trace.
fn halt(plan: &RunPlan, dir: &Path, lock: &RunLock, h: Halt) -> Result<RunStatus, RunError> {
    match h {
        Halt::Pause(why) => {
            log_event(
                plan,
                dir,
                None,
                "run.pause",
                "warn",
                json!({ "reason": why }),
            )?;
            State::Paused(why.clone())
                .write(&plan.home, &plan.study_id)
                .map_err(io)?;
            Ok(RunStatus::Paused(why))
        }
        Halt::Stop(why) => {
            control::stop(&plan.home, &plan.study_id, &plan.key, &why, lock)?;
            Ok(RunStatus::Stopped(why))
        }
    }
}

fn why_of(stop: &records::Record) -> String {
    stop.body["why"].as_str().unwrap_or_default().to_string()
}

/// One event line in the study's event log. The study is the trace; a row is a span.
fn log_event(
    plan: &RunPlan,
    dir: &Path,
    span: Option<&str>,
    name: &str,
    level: &str,
    attrs: Value,
) -> Result<(), RunError> {
    events::emit(
        &dir.join("events.jsonl"),
        Event {
            ts: crate::time::now_rfc3339(),
            trace_id: hex::encode(&plan.spec_hash[..16]),
            span_id: span.map(String::from),
            name: name.into(),
            level: level.into(),
            attrs,
        },
    )
    .map_err(io)
}

/// What the backend ticks while a row runs: a heartbeat on the plan's cadence, and a read
/// of the control file, so a stop ends the row at once.
struct RunWatch<'a> {
    plan: &'a RunPlan,
    control: &'a ControlFile,
    dir: &'a Path,
    /// Every request heard so far, merged: nothing replaces a stop.
    heard: RefCell<Option<Signal>>,
    /// The row in flight and its span id.
    row: RefCell<(u64, String)>,
    last_beat: Cell<Instant>,
}

impl RunWatch<'_> {
    fn listen(&self) -> std::io::Result<()> {
        if let Some(s) = self.control.take()? {
            let mut heard = self.heard.borrow_mut();
            *heard = Some(Signal::then(heard.take(), s));
        }
        Ok(())
    }

    fn stop_heard(&self) -> Option<String> {
        match &*self.heard.borrow() {
            Some(Signal::Stop(why)) => Some(why.clone()),
            _ => None,
        }
    }

    /// Between rows: a stop or a pause ends the run. So does a control file that cannot be
    /// read, as a pause; a stop heard earlier still wins.
    fn at_boundary(&self) -> Option<Halt> {
        let read = self.listen();
        if let Some(why) = self.stop_heard() {
            return Some(Halt::Stop(why));
        }
        match (read, &*self.heard.borrow()) {
            (Err(e), _) => Some(Halt::Pause(format!("the control file cannot be read: {e}"))),
            (_, Some(Signal::Pause)) => Some(Halt::Pause(PAUSE_REQUESTED.into())),
            _ => None,
        }
    }

    fn begin(&self, index: u64, span: &str) {
        *self.row.borrow_mut() = (index, span.into());
        self.last_beat.set(Instant::now());
    }
}

impl Watch for RunWatch<'_> {
    fn tick(&self) -> bool {
        if self.last_beat.get().elapsed() >= self.plan.heartbeat {
            self.last_beat.set(Instant::now());
            let (index, span) = self.row.borrow().clone();
            let attrs = json!({ "row": index });
            // A failed write shows at the row's end event, which ends the run.
            let _ = log_event(self.plan, self.dir, Some(&span), "heartbeat", "info", attrs);
        }
        // A failed read is retried on the next tick and pauses the run at the boundary.
        let _ = self.listen();
        self.stop_heard().is_none()
    }
}

/// The spec estimate for one row. Zero when the schedule has no rows.
fn row_estimate(plan: &RunPlan, machine: &Machine) -> Cost {
    let e = estimate(&plan.spec, machine, units(&plan.spec));
    let n = e.rows.max(1) as f64;
    Cost {
        cpu_seconds: e.cpu_hours * 3600.0 / n,
        wall_seconds: e.wall_hours * 3600.0 / n,
        usd: e.money_usd / n,
    }
}

/// Cost recorded by row records.
#[derive(Default)]
struct Tally {
    rows: u64,
    cpu: f64,
    wall: f64,
}

impl Tally {
    fn of(records: &[records::Record]) -> Tally {
        let mut t = Tally::default();
        for r in records.iter().filter(|r| r.kind == "row") {
            let num = |k: &str| r.body[k].as_f64().unwrap_or(0.0);
            t.add(num("cpu_seconds"), num("wall_seconds"));
        }
        t
    }

    fn add(&mut self, cpu: f64, wall: f64) {
        self.rows += 1;
        self.cpu += cpu;
        self.wall += wall;
    }

    fn cost(&self, rate_usd_per_hour: f64) -> Cost {
        Cost {
            cpu_seconds: self.cpu,
            wall_seconds: self.wall,
            usd: self.wall / 3600.0 * rate_usd_per_hour,
        }
    }

    /// The mean row so far; zero before the first row.
    fn mean(&self, rate_usd_per_hour: f64) -> Cost {
        let n = self.rows.max(1) as f64;
        Cost {
            cpu_seconds: self.cpu / n,
            wall_seconds: self.wall / n,
            usd: self.wall / n / 3600.0 * rate_usd_per_hour,
        }
    }
}

/// One row in its own span: `row.start`, the attempts, `row.end`. `Some` ends the run.
fn run_row(
    plan: &RunPlan,
    row: &Row,
    dir: &Path,
    log: &Path,
    tally: &mut Tally,
    watch: &RunWatch,
) -> Result<Option<Halt>, RunError> {
    let span = events::new_span_id();
    watch.begin(row.index, &span);
    let start = json!({ "row": row.index });
    log_event(plan, dir, Some(&span), "row.start", "info", start)?;
    let (exit, halt) = attempt_row(plan, row, dir, log, tally, watch)?;
    let end = json!({ "row": row.index, "exit": exit });
    log_event(plan, dir, Some(&span), "row.end", "info", end)?;
    Ok(halt)
}

/// A row's attempts, retries included, then its record. Returns the recorded exit, or
/// `stopped` or `paused` for a row that ends the run without a record.
fn attempt_row(
    plan: &RunPlan,
    row: &Row,
    dir: &Path,
    log: &Path,
    tally: &mut Tally,
    watch: &RunWatch,
) -> Result<(String, Option<Halt>), RunError> {
    let mut attempts = 0u32;
    let (mut cpu, mut wall) = (0.0, 0.0);
    let outcome = loop {
        attempts += 1;
        let job = job_for(plan, row, dir)?;
        let o = plan
            .backend
            .run_row(&job, watch)
            .unwrap_or_else(|e| RowOutcome {
                exit: Exit::Infra(e.to_string()),
                cpu_seconds: 0.0,
                wall_seconds: 0.0,
                output_hash: String::new(),
                selftest_ok: false,
            });
        cpu += o.cpu_seconds;
        wall += o.wall_seconds;
        // A stopped row is not data: it gets no record, and the study runs no further row.
        if let Some(why) = watch.stop_heard() {
            return Ok(("stopped".into(), Some(Halt::Stop(why))));
        }
        if !matches!(o.exit, Exit::Infra(_)) || attempts > MAX_RETRIES {
            break o;
        }
        if let Probe::Unavailable(why) = plan.backend.probe() {
            let why = format!("backend unavailable: {why}");
            return Ok(("paused".into(), Some(Halt::Pause(why))));
        }
    };
    let exit = match &outcome.exit {
        Exit::Ok => "ok".to_string(),
        Exit::Payload(c) => format!("payload:{c}"),
        Exit::Infra(_) => "void:infra".to_string(),
        Exit::Stopped => "void:stopped".to_string(),
    };
    records::append(
        log,
        &plan.key,
        RecordDraft {
            study: plan.study_id.clone(),
            row: Some(row.index),
            kind: "row".into(),
            body: json!({
                "condition": row.condition,
                "unit": row.unit,
                "repeat": row.repeat,
                "seed": row.seed_hex,
                "isolation": plan.backend.level(),
                "payload_sha256": plan.payload_sha256,
                "data_hashes": plan.data_hashes,
                "cpu_seconds": cpu,
                "wall_seconds": wall,
                "output_hash": outcome.output_hash,
                "exit": exit,
                "attempts": attempts,
            }),
        },
    )?;
    tally.add(cpu, wall);
    plan.meter.add_row(cpu);
    let halt = match outcome.exit {
        Exit::Infra(why) => Some(Halt::Pause(format!(
            "row {} voided by infrastructure faults: {why}",
            row.index
        ))),
        _ => None,
    };
    Ok((exit, halt))
}

/// The job for one attempt, with a fresh output dir. An old one is renamed, never deleted.
fn job_for(plan: &RunPlan, row: &Row, dir: &Path) -> Result<RowJob, RunError> {
    let index = row.index.to_string();
    no_links(dir, &["run", "rows", &index])?;
    let row_dir = std::path::absolute(dir.join("run").join("rows").join(&index)).map_err(io)?;
    let out = row_dir.join("out");
    std::fs::create_dir_all(&row_dir).map_err(io)?;
    if out.symlink_metadata().is_ok() {
        let moved = (0u32..)
            .map(|k| row_dir.join(format!("out.crashed-{k}")))
            .find(|p| p.symlink_metadata().is_err())
            .ok_or_else(|| io("no free name for the old output dir"))?;
        std::fs::rename(&out, moved).map_err(io)?;
    }
    std::fs::create_dir(&out).map_err(io)?;
    let input_dir = std::path::absolute(dir.join("data")).map_err(io)?;
    let mut seed = [0u8; 32];
    hex::decode_to_slice(&row.seed_hex, &mut seed).map_err(io)?;
    let text = |p: &Path| p.to_string_lossy().into_owned();
    Ok(RowJob {
        argv: vec![
            text(&plan.payload),
            "--condition".into(),
            row.condition.clone(),
            "--unit".into(),
            row.unit.clone(),
            "--repeat".into(),
            row.repeat.to_string(),
        ],
        env: vec![
            ("PATH".into(), PATH_ENV.into()),
            ("IGNORARIUM_SEED".into(), row.seed_hex.clone()),
            ("IGNORARIUM_ROW".into(), row.index.to_string()),
            ("IGNORARIUM_IN".into(), text(&input_dir)),
            ("IGNORARIUM_OUT".into(), text(&out)),
        ],
        input_dir,
        output_dir: out,
        seed,
        wall_timeout: wall_timeout(plan.spec.design.row_seconds),
        rootfs: None,
    })
}

/// Ten times the row estimate, and never under a minute.
fn wall_timeout(row_seconds: f64) -> Duration {
    Duration::try_from_secs_f64(row_seconds * TIMEOUT_FACTOR)
        .unwrap_or(MIN_TIMEOUT)
        .max(MIN_TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_is_ten_times_the_estimate_with_a_floor() {
        assert_eq!(wall_timeout(1.0), Duration::from_secs(60));
        assert_eq!(wall_timeout(6.0), Duration::from_secs(60));
        assert_eq!(wall_timeout(7.0), Duration::from_secs(70));
        assert_eq!(wall_timeout(f64::NAN), Duration::from_secs(60));
    }
}
