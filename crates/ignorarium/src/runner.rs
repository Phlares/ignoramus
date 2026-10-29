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
use std::collections::{BTreeMap, BTreeSet};
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
    let (spec, spec_hash, hash_text) = stored_spec(&dir)?;
    if register.body["spec_hash"].as_str() != Some(hash_text.as_str()) {
        return Err(RunError::SpecChanged);
    }

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
    let rows = expand_rows(&spec, &spec_hash)?;
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

/// The stored spec with defaults filled, its hash bytes, and its hash text.
fn stored_spec(dir: &Path) -> Result<(Spec, [u8; 32], String), RunError> {
    let fault = |e: &dyn fmt::Display| RunError::Spec(e.to_string());
    let raw = spec::load(&dir.join("spec.toml")).map_err(|e| fault(&e))?;
    let mut value = raw.value;
    apply_defaults(&mut value);
    let spec = Spec::from_value(&value).map_err(|e| fault(&e))?;
    let hash = spec::hash_bytes(&raw.hash)
        .ok_or_else(|| RunError::Spec(format!("bad spec hash {}", raw.hash)))?;
    Ok((spec, hash, raw.hash))
}

fn expand_rows(spec: &Spec, hash: &[u8; 32]) -> Result<Vec<Row>, RunError> {
    schedule::expand(spec, hash, &schedule::units_from_spec(spec))
        .map_err(|e| RunError::Spec(e.to_string()))
}

/// How far a study has run, from its signed records and its stored spec.
pub struct Progress {
    pub spec: Spec,
    pub spec_hash: [u8; 32],
    pub total: usize,
    pub done: usize,
    /// Condition id to (rows done, rows scheduled).
    pub conditions: BTreeMap<String, (u64, u64)>,
    /// Rows whose exit is not `ok`.
    pub voided: u64,
    /// Attempts beyond the first, over all recorded rows.
    pub retried: u64,
    /// Spend in the records, money at the machine rate.
    pub cost: Cost,
}

pub fn progress(
    home: &Home,
    study_id: &str,
    all: &[records::Record],
) -> Result<Progress, RunError> {
    let (spec, spec_hash, _) = stored_spec(&home.study_dir(study_id))?;
    let machine = machine::load(&home.root).map_err(io)?;
    let rows = expand_rows(&spec, &spec_hash)?;
    let tally = Tally::of(all);
    let mut conditions: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for r in &rows {
        conditions.entry(r.condition.clone()).or_default().1 += 1;
    }
    let mut seen = BTreeSet::new();
    for r in all.iter().filter(|r| r.kind == "row") {
        if let (Some(i), Some(c)) = (r.row, r.body["condition"].as_str())
            && seen.insert(i)
        {
            conditions.entry(c.to_string()).or_default().0 += 1;
        }
    }
    Ok(Progress {
        spec,
        spec_hash,
        total: rows.len(),
        done: seen.len(),
        conditions,
        voided: tally.voided,
        retried: tally.retried,
        cost: tally.cost(machine.rate_usd_per_hour),
    })
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
    let own = own_label(id);
    let mut new = Vec::new();
    for (path, hashes) in late {
        let uses = log.uses(&hashes.0, &hashes.1)?;
        if let Some((i, leaf)) = uses.iter().find(|(_, l)| !is_own(l, &own)) {
            let label = match (leaf.kind.as_str(), leaf.label.strip_prefix(OWN_PREFIX)) {
                ("legacy", Some(study)) => format!("study {study}"),
                ("legacy", None) => format!("phase {}", leaf.label),
                _ => format!("registered plan, leaf {i}"),
            };
            return Err(RunError::DataReused { path, label });
        }
        if uses.is_empty() {
            new.push(hashes);
        }
    }
    if !new.is_empty() {
        log.append_legacy(&own, new, key)?;
    }
    Ok(all)
}

/// The label of the late-data leaves a study appends. A study id holds no `:`, and
/// `import-history` refuses a phase name with this prefix, so no other leaf has it.
pub const OWN_PREFIX: &str = "study:";

fn own_label(id: &str) -> String {
    format!("{OWN_PREFIX}{id}")
}

/// A late-data leaf this study appended. A plan leaf is never its own use.
fn is_own(leaf: &Leaf, own: &str) -> bool {
    leaf.kind == "legacy" && leaf.label == own
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
/// A row a dead box left running is killed first. At each row boundary the control file is
/// read: a stop ends the run with the stop trace, a pause ends it at that boundary. Then the
/// meter is read: the run pauses instead of starting a row when a cap is reached or the row
/// would reach one. While a row runs, the backend ticks the watch, which emits heartbeats and
/// ends the row on a stop.
pub fn run(plan: &RunPlan) -> Result<RunStatus, RunError> {
    no_links(&plan.home.study_dir(&plan.study_id), &["run"])?;
    let lock = RunLock::try_take(&plan.home, &plan.study_id)
        .map_err(io)?
        .ok_or(RunError::Busy)?;
    let result = run_locked(plan, &lock);
    drop(lock);
    // A stop sent while this run held the lock, after its last look or before an error,
    // found the lock taken: finish it here, on every path. A failure of this check leaves
    // the run's own result, which already ended the run safely.
    let late = control::stop_if_idle(&plan.home, &plan.study_id, &plan.key);
    match (result, late) {
        (Ok(_), Ok(Some(r))) => Ok(RunStatus::Stopped(why_of(&r))),
        (result, _) => result,
    }
}

fn run_locked(plan: &RunPlan, lock: &RunLock) -> Result<RunStatus, RunError> {
    let dir = plan.home.study_dir(&plan.study_id);
    let log = dir.join("records.jsonl");
    let machine = machine::load(&plan.home.root).map_err(io)?;
    let all = records::read_all(&log, &plan.key.verifying_key())?;
    if let Some(r) = all.iter().find(|r| r.kind == "stop") {
        let why = why_of(r);
        return halt(
            plan,
            &dir,
            lock,
            Halt::Stop {
                why,
                in_flight: None,
            },
        );
    }
    match control::end_orphan(&plan.home, &plan.study_id) {
        Ok(Some(o)) => {
            let attrs = json!({ "row": o.row, "exit": "orphaned", "note": o.note });
            log_event(plan, &dir, Some(&o.span), "row.end", "info", attrs)?;
        }
        Ok(None) => {}
        Err(e) => {
            let why = format!("cannot end the row a dead box left: {e}");
            return halt(plan, &dir, lock, Halt::Pause(why));
        }
    }
    State::Running
        .write(&plan.home, &plan.study_id)
        .map_err(io)?;
    let control = ControlFile::of(&plan.home, &plan.study_id);
    let watch = RunWatch {
        plan,
        control: &control,
        dir: &dir,
        row: RefCell::new((0, String::new())),
        last_beat: Cell::new(Instant::now()),
        ended: RefCell::new(None),
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
        if let Some(h) = at_boundary(&control) {
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

/// Between rows: a stop or a pause ends the run, and so does a control file that cannot be
/// read, as a pause.
fn at_boundary(control: &ControlFile) -> Option<Halt> {
    match control.take() {
        Ok(Some(Signal::Stop(why))) => Some(Halt::Stop {
            why,
            in_flight: None,
        }),
        Ok(Some(Signal::Pause)) => Some(Halt::Pause(PAUSE_REQUESTED.into())),
        Ok(_) => None,
        Err(e) => Some(Halt::Pause(format!("the control file cannot be read: {e}"))),
    }
}

/// Why a run ends before its last row.
enum Halt {
    Pause(String),
    /// `in_flight` names the row the stop killed.
    Stop {
        why: String,
        in_flight: Option<u64>,
    },
}

impl Halt {
    /// The `row.end` exit of a row this halt ends early.
    fn exit(&self) -> &'static str {
        match self {
            Halt::Pause(_) => "paused",
            Halt::Stop { .. } => "stopped",
        }
    }

    /// The reason a spend record gives for a row this halt ends early.
    fn reason(&self) -> String {
        match self {
            Halt::Pause(why) => why.clone(),
            Halt::Stop { why, .. } => format!("stopped: {why}"),
        }
    }
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
        Halt::Stop { why, in_flight } => {
            lock.stop(&plan.key, &why, in_flight)?;
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
            trace_id: events::trace_id(&plan.spec_hash),
            span_id: span.map(String::from),
            name: name.into(),
            level: level.into(),
            attrs,
        },
    )
    .map_err(io)
}

/// What the backend tells the box while a row runs. Each tick emits a heartbeat on the
/// plan's cadence and looks at the control file without waiting; the file stays the one
/// place requests live, and only a boundary takes them.
struct RunWatch<'a> {
    plan: &'a RunPlan,
    control: &'a ControlFile,
    dir: &'a Path,
    /// The row in flight and its span id.
    row: RefCell<(u64, String)>,
    last_beat: Cell<Instant>,
    /// Why this watch ended the row early, once it has: a stop seen, or a fault.
    ended: RefCell<Option<Halt>>,
}

impl RunWatch<'_> {
    fn begin(&self, index: u64, span: &str) {
        *self.row.borrow_mut() = (index, span.into());
        self.last_beat.set(Instant::now());
    }

    /// End the row early for `h`; the first reason stands.
    fn end(&self, h: Halt) {
        self.ended.borrow_mut().get_or_insert(h);
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
        // A failed or busy read is retried on the next tick; a boundary pauses on a failure.
        if let Ok(Some(Signal::Stop(why))) = self.control.peek_now() {
            let in_flight = Some(self.row.borrow().0);
            self.end(Halt::Stop { why, in_flight });
        }
        self.ended.borrow().is_none()
    }

    fn started(&self, pid: u32) {
        let (index, span) = self.row.borrow().clone();
        let home = &self.plan.home;
        if let Err(e) = control::row_started(home, &self.plan.study_id, index, &span, pid) {
            // A row the box cannot find again after a crash must not run.
            self.end(Halt::Pause(format!("cannot record the running row: {e}")));
        }
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

/// Cost in the signed records: every attempt's spend, from `row` and `spend` records.
/// `rows` counts completed rows only, so the mean stays per completed row.
#[derive(Default)]
struct Tally {
    rows: u64,
    cpu: f64,
    wall: f64,
    voided: u64,
    retried: u64,
}

impl Tally {
    fn of(records: &[records::Record]) -> Tally {
        let mut t = Tally::default();
        for r in records
            .iter()
            .filter(|r| r.kind == "row" || r.kind == "spend")
        {
            let num = |k: &str| r.body[k].as_f64().unwrap_or(0.0);
            t.cpu += num("cpu_seconds");
            t.wall += num("wall_seconds");
            t.rows += u64::from(r.kind == "row");
            if r.kind == "row" {
                t.voided += u64::from(r.body["exit"] != "ok");
                t.retried += r.body["attempts"].as_u64().unwrap_or(1).saturating_sub(1);
            }
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
/// `stopped` or `paused` for a row that ends the run with only a spend record.
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
        control::row_ended(&plan.home, &plan.study_id);
        cpu += o.cpu_seconds;
        wall += o.wall_seconds;
        // A row ended early is not data: its spend is recorded, and the run ends.
        if let Some(h) = watch.ended.borrow_mut().take() {
            return ended_early(plan, log, row, (cpu, wall), h);
        }
        // A stop nobody asked for is a backend fault: void the row and pause, no retry.
        if o.exit == Exit::Stopped {
            let exit = Exit::Infra("the backend stopped the row with no stop requested".into());
            break RowOutcome { exit, ..o };
        }
        if !matches!(o.exit, Exit::Infra(_)) || attempts > MAX_RETRIES {
            break o;
        }
        if let Probe::Unavailable(why) = plan.backend.probe() {
            let h = Halt::Pause(format!("backend unavailable: {why}"));
            return ended_early(plan, log, row, (cpu, wall), h);
        }
    };
    let exit = match &outcome.exit {
        Exit::Ok => "ok".to_string(),
        Exit::Payload(c) => format!("payload:{c}"),
        Exit::Infra(_) | Exit::Stopped => "void:infra".to_string(),
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

/// A row that ends the run without a row record: its attempts' spend goes into one signed
/// `spend` record, so a resume counts it.
fn ended_early(
    plan: &RunPlan,
    log: &Path,
    row: &Row,
    (cpu, wall): (f64, f64),
    h: Halt,
) -> Result<(String, Option<Halt>), RunError> {
    records::append(
        log,
        &plan.key,
        RecordDraft {
            study: plan.study_id.clone(),
            row: Some(row.index),
            kind: "spend".into(),
            body: json!({ "cpu_seconds": cpu, "wall_seconds": wall, "reason": h.reason() }),
        },
    )?;
    plan.meter.add_row(cpu);
    Ok((h.exit().into(), Some(h)))
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
