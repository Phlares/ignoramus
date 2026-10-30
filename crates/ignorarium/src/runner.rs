//! The run engine: writes the schedule, runs each row on a backend, records each row.
//! A row is done when its signed record exists. A crash before the record reruns the row.
use crate::backend::rootfs;
use crate::backend::{self, Backend, Exit, Probe, RowJob, RowOutcome, Watch};
use crate::budget::{Alerts, Cost, CostMeter, Decision, HostMeter, decide, dimension_name};
use crate::control::{self, ControlError, ControlFile, RunLock, Signal, State};
use crate::derived::{REGISTER, Register};
use crate::environ::{Builder, Built, Recipe};
use crate::estimate::{apply_defaults, estimate, units};
use crate::events::{self, Event};
use crate::home::Home;
use crate::inputs::{self, Found};
use crate::machine::{self, Machine};
use crate::payload::{self, PayloadError};
use crate::planlog::store::{Leaf, LogError, PlanLog};
use crate::records::{self, RecordDraft, RecordError, VerifyError};
use crate::rules::input_path;
use crate::schedule::{self, Row, Schedule, ScheduleError};
use crate::spec::model::{Role, Spec};
use crate::store::Store;
use crate::view::{self, ViewError};
use crate::{keys, spec, sys};
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
const EVENT_LOG: &str = "the event log cannot be written";
const INTERRUPTED: &str = "interrupted by a signal";
const REAPED: &str = "SIGCHLD is ignored; the box cannot see a row end";
/// The payload, relative to the study folder.
const PAYLOAD: &str = "payload/run";
/// The recipe folder, relative to the study folder.
const RECIPE: &str = "env";
/// The environment name of a run on the host root.
const HOST: &str = "host";

pub struct RunPlan {
    pub home: Home,
    pub study_id: String,
    pub spec: Spec,
    pub spec_hash: [u8; 32],
    pub schedule: Schedule,
    pub backend: Box<dyn Backend>,
    pub key: SigningKey,
    pub inputs: InputHashes,
    /// The input view rows see: `<study>/in`, one link per input present.
    pub view: PathBuf,
    /// The scan of the view that `prepare` took before every file in it hashed to its digest.
    pub view_seen: view::Scan,
    pub payload: PathBuf,
    pub payload_sha256: String,
    /// The environment root rows run in; `None` is the host root.
    pub rootfs: Option<PathBuf>,
    /// `"host"`, or the store digest of the environment image.
    pub environment: String,
    /// Cumulative cost, read before each row.
    pub meter: Box<dyn CostMeter>,
    /// Interval between `heartbeat` events while a row runs.
    pub heartbeat: Duration,
}

/// Each input path with its `(exact, normalized)` hashes, in spec order; `None` while the
/// input is absent.
pub type InputHashes = Vec<(String, Option<(String, String)>)>;

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
    /// The `register` record exists but does not parse.
    BadRegister(String),
    /// The stored spec no longer hashes to the registered hash.
    SpecChanged,
    /// The study's payload copy no longer hashes to the register record's `payload`.
    PayloadChanged,
    NoPayload(String),
    /// A registered input's stored copy is not in the store, by input path.
    InputMissing(String),
    /// A registered input's stored copy no longer has its registered bytes, by input path.
    InputChanged(String),
    /// `pins.environment` names an image that is not in the store.
    EnvironmentMissing(String),
    /// `pins.environment` is not a store digest (64 lowercase hex characters).
    EnvironmentPin(String),
    /// The environment cannot be built or extracted.
    Environment(String),
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
            RunError::BadRegister(id) => write!(
                f,
                "the register record of {id} does not parse; register the plan again"
            ),
            RunError::SpecChanged => write!(f, "the stored spec differs from the registered hash"),
            RunError::PayloadChanged => write!(f, "the payload differs from the registered copy"),
            RunError::NoPayload(e) => write!(f, "cannot read the payload: {e}"),
            RunError::InputMissing(p) => {
                write!(
                    f,
                    "the store no longer holds the registered copy of input {p}"
                )
            }
            RunError::InputChanged(p) => write!(
                f,
                "the stored copy of input {p} no longer has the bytes it was registered with"
            ),
            RunError::EnvironmentMissing(d) => {
                write!(f, "the pinned environment {d} is not in the store")
            }
            RunError::EnvironmentPin(p) => {
                write!(f, "pins.environment {p:?} is not a store digest")
            }
            RunError::Environment(e) => write!(f, "environment: {e}"),
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

/// Load a registered study, check its payload copy and its stored inputs, build its input
/// view, gate its late data, write its schedule, and pick the highest backend that meets
/// `scope.min_isolation`. Every input in the view is hashed in full here, once per `run` or
/// `resume`; before each attempt the run only compares the view's stamps (see `stale`).
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
        .find(|r| r.kind == REGISTER)
        .ok_or_else(|| RunError::NotRegistered(study_id.into()))?;
    let register = Register::of(register).ok_or_else(|| RunError::BadRegister(study_id.into()))?;
    let (spec, spec_hash, hash_text) = stored_spec(&dir)?;
    if register.spec_hash != hash_text {
        return Err(RunError::SpecChanged);
    }

    let have: Vec<u8> = backends.iter().map(|b| b.level()).collect();
    let need = spec.scope.min_isolation;
    let backend = backends
        .into_iter()
        .filter(|b| b.level() >= need)
        .max_by_key(|b| b.level())
        .ok_or(RunError::NoBackend { need, have })?;

    let (payload, payload_sha256) = read_payload(&dir)?;
    if payload_sha256 != register.payload {
        return Err(RunError::PayloadChanged);
    }
    let store = Store::open(&home.root).map_err(io)?;
    let (rootfs, environment) = if backend.level() >= 1 {
        let pin = spec.pins.environment.trim();
        let found = environment(&store, &dir, pin, Builder::detect().as_ref())?;
        if let Some(b) = &found.built {
            let attrs = json!({
                "recipe_hash": b.recipe_hash,
                "image_digest": b.image_digest,
                "store_digest": b.store_digest,
                "pinned": b.pinned,
            });
            let level = if b.pinned { "info" } else { "warn" };
            emit_event(&dir, &spec_hash, None, "env.build", level, attrs)?;
        }
        (found.rootfs, found.name)
    } else {
        (None, HOST.into())
    };

    let leaf = register.leaf;
    let log = match register.log.as_str() {
        l @ ("planlog" | "testlog") => l,
        other => {
            return Err(RunError::Records(format!(
                "register record names log {other:?}"
            )));
        }
    };
    let machine = machine::load(&home.root).map_err(io)?;
    let inputs = held_inputs(&store, &key, &spec, &register)?;
    let present = present(&inputs);
    let view = view::build(&store, &dir, &present).map_err(view_error)?;
    let view_seen = view::verify(&view, &present).map_err(view_error)?;
    gate_late_data(home, &spec, log, leaf, &key, &inputs)?;
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
        inputs,
        view,
        view_seen,
        payload,
        payload_sha256,
        rootfs,
        environment,
        meter: Box::new(HostMeter::new(machine.rate_usd_per_hour)),
        heartbeat: HEARTBEAT,
    })
}

/// Where rows run: the root folder (`None` is the host's) and its name.
pub struct Environment {
    pub rootfs: Option<PathBuf>,
    /// `"host"`, or the store digest of the image.
    pub name: String,
    /// What the build made, when this call built the image.
    pub built: Option<Built>,
}

/// The environment for a level 1 row. A non-empty `pin` is the store digest of a saved
/// image, used as is. Otherwise the recipe in `<study>/env/` is built with `builder` and its
/// image used; with no recipe the host root is used.
pub fn environment(
    store: &Store,
    study: &Path,
    pin: &str,
    builder: Option<&Builder>,
) -> Result<Environment, RunError> {
    let given = pin;
    let pin = pin.strip_prefix("sha256:").unwrap_or(pin);
    let (digest, built) = if !pin.is_empty() {
        if store.path(pin).is_err() {
            return Err(RunError::EnvironmentPin(given.into()));
        }
        if !store.has(pin) {
            return Err(RunError::EnvironmentMissing(pin.into()));
        }
        (pin.to_string(), None)
    } else {
        no_links(study, &[RECIPE])?;
        let recipe = study.join(RECIPE);
        if !recipe.is_dir() {
            return Ok(Environment {
                rootfs: None,
                name: HOST.into(),
                built: None,
            });
        }
        let builder = builder.ok_or_else(|| {
            RunError::Environment(
                concat!(
                    "the study has an env/ recipe but neither docker nor podman is on PATH; ",
                    "build the image elsewhere and pin its store digest in pins.environment"
                )
                .into(),
            )
        })?;
        Recipe::load(&recipe).map_err(|e| {
            RunError::Environment(format!("cannot read env/Dockerfile and env/uv.lock: {e}"))
        })?;
        let built = crate::environ::build(builder, &recipe, store)
            .map_err(|e| RunError::Environment(e.to_string()))?;
        (built.store_digest.clone(), Some(built))
    };
    let root =
        rootfs::from_store(store, &digest).map_err(|e| RunError::Environment(e.to_string()))?;
    Ok(Environment {
        rootfs: Some(root),
        name: digest,
        built,
    })
}

/// The payload's absolute `run` path and the hash of its whole folder. A link anywhere in the
/// folder is refused.
fn read_payload(dir: &Path) -> Result<(PathBuf, String), RunError> {
    let hash = payload::tree_hash(&dir.join("payload")).map_err(|e| match e {
        PayloadError::Unsafe(p) => RunError::Unsafe(p),
        e => RunError::NoPayload(e.to_string()),
    })?;
    let path = std::path::absolute(dir.join(PAYLOAD)).map_err(io)?;
    Ok((path, hash))
}

/// The path and digest of each input present.
fn present(inputs: &InputHashes) -> Vec<(String, String)> {
    inputs
        .iter()
        .filter_map(|(p, h)| Some((p.clone(), h.as_ref()?.0.clone())))
        .collect()
}

fn view_error(e: ViewError) -> RunError {
    match e {
        ViewError::Missing(p) => RunError::InputMissing(p),
        ViewError::Changed(p) => RunError::InputChanged(p),
        ViewError::Unsafe(p) => RunError::Unsafe(p),
        ViewError::Io(e) => io(format!("input view: {e}")),
    }
}

/// Each input's hashes. A registered input comes from its stored copy, which must still be
/// in the store with its size. An input absent at registration is read from the study root
/// the register record names, never from the box's copy of the study, and imported when it
/// is there now; it is then late data for `gate_late_data`. A pin always resolves at
/// registration (E-HSH-001), so such an input is unpinned.
fn held_inputs(
    store: &Store,
    key: &SigningKey,
    spec: &Spec,
    register: &Register,
) -> Result<InputHashes, RunError> {
    let root = Path::new(&register.root);
    let source = format!("run {}", spec.study.id);
    let mut all = Vec::new();
    for i in &spec.inputs {
        let digest = register.inputs.iter().find(|(p, _)| *p == i.path);
        let hashes = match digest {
            // The blob first: `resolve` reads a missing blob and a resized one alike.
            Some((path, digest)) if !store.has(digest) => {
                return Err(RunError::InputMissing(path.clone()));
            }
            Some((path, digest)) => match inputs::resolve(store, root, path, digest) {
                Found::Stored { digest, meta } => Some((digest, meta.normalized_sha256)),
                Found::Damaged(_) => return Err(RunError::InputChanged(path.clone())),
                _ => return Err(RunError::InputMissing(path.clone())),
            },
            None => match inputs::resolve(store, root, &i.path, &i.sha256) {
                Found::Stored { digest, meta } => Some((digest, meta.normalized_sha256)),
                Found::File { .. } => {
                    let file = input_path(root, &i.path)
                        .ok_or_else(|| RunError::Unsafe(i.path.clone()))?;
                    let (digest, meta) = store.import_signed(&file, key, &source).map_err(io)?;
                    Some((digest, meta.normalized_sha256))
                }
                Found::Missing | Found::Unsafe | Found::Damaged(_) => None,
            },
        };
        all.push((i.path.clone(), hashes));
    }
    Ok(all)
}

/// What no longer matches what `prepare` saw, as a pause reason. The payload is hashed in
/// full, and an unreadable or linked payload counts as changed. The input view is only
/// scanned: a file whose device, inode, size or modification time differs is named; an entry
/// added or removed elsewhere names the view. A large input is hashed in full at `prepare`
/// only.
fn stale(plan: &RunPlan, dir: &Path) -> Option<String> {
    let mut changed = Vec::new();
    if !read_payload(dir).is_ok_and(|(_, h)| h == plan.payload_sha256) {
        changed.push("payload");
    }
    let now = view::scan(&plan.view).unwrap_or_default();
    if now != plan.view_seen {
        let named: Vec<&str> = plan
            .inputs
            .iter()
            .filter(|(p, h)| {
                let k = view::key(p);
                h.is_some() && now.get(&k) != plan.view_seen.get(&k)
            })
            .map(|(p, _)| p.as_str())
            .collect();
        if named.is_empty() {
            changed.push("the input view");
        }
        changed.extend(named);
    }
    (!changed.is_empty()).then(|| format!("{} changed after the run began", changed.join(", ")))
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
    /// Attempts, recorded as a row or as spend, whose CPU count could not be read.
    pub unknown_cpu: u64,
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
        unknown_cpu: tally.unknown_cpu,
        retried: tally.retried,
        cost: tally.cost(machine.rate_usd_per_hour),
    })
}

/// A selection or confirmation input that the registered plan leaf did not hold is late:
/// refuse if another study used it, else record it under this study's id once. `all` are the
/// inputs' hashes in spec order. Every lookup finishes before the append takes its lock.
fn gate_late_data(
    home: &Home,
    spec: &Spec,
    log_name: &str,
    leaf_index: u64,
    key: &SigningKey,
    all: &InputHashes,
) -> Result<(), RunError> {
    let id = spec.study.id.as_str();
    let log = PlanLog::open(&home.root.join(log_name))?;
    let registered = log
        .leaf(leaf_index)?
        .ok_or_else(|| RunError::NotRegistered(id.into()))?
        .data_hashes;
    let mut late = Vec::new();
    for (input, (_, hashes)) in spec.inputs.iter().zip(all) {
        if let Some(hashes) = hashes
            && matches!(input.role, Role::Selection | Role::Confirmation)
            && !registered.contains(hashes)
        {
            late.push((input.path.clone(), hashes.clone()));
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
    Ok(())
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
/// gate runs before each attempt, retries included: the run pauses instead of starting it
/// when a cap is reached or the attempt would reach one, or when the payload or an input
/// changed. While a row runs, the backend ticks the watch, which emits heartbeats and ends
/// the row on a stop. Once the caller has called `sys::catch_interrupts`, SIGINT, SIGTERM or
/// SIGHUP ends the row as a stop does, and the run pauses. The library leaves signal handling
/// to its caller.
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
            None,
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
            return halt(plan, &dir, lock, Halt::Pause(why), None);
        }
    }
    State::Running
        .write(&plan.home, &plan.study_id)
        .map_err(io)?;
    let launch = json!({
        "isolation": plan.backend.level(),
        "launcher": plan.backend.launcher(),
        "environment": plan.environment,
    });
    let resumed = all.iter().any(|r| r.kind == "row" || r.kind == "spend");
    let name = if resumed { "run.resume" } else { "run.start" };
    log_event(plan, &dir, None, name, "info", launch)?;
    let control = ControlFile::of(&plan.home, &plan.study_id);
    let watch = RunWatch {
        plan,
        control: &control,
        dir: &dir,
        row: RefCell::new((0, String::new())),
        last_beat: Cell::new(Instant::now()),
        ended: RefCell::new(None),
        failed: RefCell::new(None),
    };
    let done: BTreeSet<u64> = all
        .iter()
        .filter(|r| r.kind == "row")
        .filter_map(|r| r.row)
        .collect();
    let mut books = Books::open(plan, &all, &machine);
    for row in plan
        .schedule
        .rows
        .iter()
        .filter(|r| !done.contains(&r.index))
    {
        if let Some(h) = at_boundary(&control) {
            return halt(plan, &dir, lock, h, None);
        }
        if let Some(h) = gate(plan, &dir, &mut books)? {
            return halt(plan, &dir, lock, h, None);
        }
        let h = run_row(plan, row, &dir, &log, &mut books, &watch)?;
        // A failed event write is a failed internal check: the run pauses after the row.
        let failed = watch.failed.take();
        if let Some(h) = h.or_else(|| failed.is_some().then(|| Halt::Pause(EVENT_LOG.into()))) {
            return halt(plan, &dir, lock, h, failed);
        }
    }
    State::Done.write(&plan.home, &plan.study_id).map_err(io)?;
    Ok(RunStatus::Done)
}

/// Before each attempt: the budget has room, the payload and inputs are the bytes the run
/// began with, the box can see a child end, and no signal arrived. Otherwise the run pauses
/// and the attempt never starts. The signal comes last, so one that lands during the
/// re-hash still counts.
fn gate(plan: &RunPlan, dir: &Path, books: &mut Books) -> Result<Option<Halt>, RunError> {
    let budget = books.check(plan, dir)?;
    Ok(budget
        .or_else(|| stale(plan, dir).map(Halt::Pause))
        .or_else(|| blind_to_exits().map(Halt::Pause))
        .or_else(|| sys::interrupted().then(|| Halt::Pause(INTERRUPTED.into()))))
}

/// Why the box cannot see a row end, when it cannot. The library reads the SIGCHLD action and
/// leaves it as its caller set it.
fn blind_to_exits() -> Option<String> {
    match sys::sigchld_ignored() {
        Ok(false) => None,
        Ok(true) => Some(REAPED.into()),
        Err(e) => Some(format!("cannot read the SIGCHLD action: {e}")),
    }
}

/// This session's spend: the signed records' tally, the alerts already fired, and the row
/// estimate. Every attempt's spend goes into the tally and the meter together.
struct Books {
    tally: Tally,
    alerts: Alerts,
    per_row: Cost,
    rate_usd_per_hour: f64,
}

impl Books {
    /// Start from the records read under the run lock; the meter starts there too.
    fn open(plan: &RunPlan, all: &[records::Record], machine: &Machine) -> Books {
        let tally = Tally::of(all);
        let prior = tally.cost(machine.rate_usd_per_hour);
        plan.meter.set_prior(prior);
        Books {
            tally,
            // Only spend from earlier sessions stays silent; this session's first reading may alert.
            alerts: Alerts::seeded(&prior, &plan.spec.scope.budget),
            per_row: row_estimate(plan, machine),
            rate_usd_per_hour: machine.rate_usd_per_hour,
        }
    }

    /// Read the meter, emit the alerts it crossed, and pause when a cap is reached or the
    /// next attempt would reach one.
    fn check(&mut self, plan: &RunPlan, dir: &Path) -> Result<Option<Halt>, RunError> {
        let budget = &plan.spec.scope.budget;
        let sampled = plan.meter.sample();
        let now = sampled.clone().unwrap_or_default();
        if let Ok(c) = &sampled {
            for (dim, fraction) in self.alerts.fresh(c, budget) {
                let attrs = json!({ "dimension": dimension_name(dim), "fraction": fraction });
                log_event(plan, dir, None, "budget.alert", "warn", attrs)?;
            }
        }
        let next = self
            .per_row
            .larger(&self.tally.mean(self.rate_usd_per_hour));
        Ok(match decide(&now, &next, budget, sampled.map(|_| ())) {
            Decision::Pause(why) => Some(Halt::Pause(why.to_string())),
            Decision::Continue => None,
        })
    }

    /// An attempt's spend; `completed` when its row record holds it.
    fn add(&mut self, plan: &RunPlan, o: &RowOutcome, completed: bool) {
        self.tally.add(o.cpu_seconds, o.wall_seconds, completed);
        plan.meter.add_row(o.cpu_seconds);
    }
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

/// End the run. A pause writes the paused state, then `run.pause`; a stop writes the stop
/// trace, its record first. `failed` is an event write that failed during the row this halt
/// ends. When it is set, or `run.pause` cannot be written, the pause also stays a pending
/// request, so no run goes past it without a resume, and the failure is the result.
fn halt(
    plan: &RunPlan,
    dir: &Path,
    lock: &RunLock,
    h: Halt,
    failed: Option<RunError>,
) -> Result<RunStatus, RunError> {
    match h {
        Halt::Pause(why) => {
            State::Paused(why.clone())
                .write(&plan.home, &plan.study_id)
                .map_err(io)?;
            let attrs = json!({ "reason": why });
            let logged = log_event(plan, dir, None, "run.pause", "warn", attrs);
            match failed.or(logged.err()) {
                None => Ok(RunStatus::Paused(why)),
                Some(e) => {
                    ControlFile::of(&plan.home, &plan.study_id)
                        .request_pause()
                        .map_err(io)?;
                    Err(e)
                }
            }
        }
        Halt::Stop { why, in_flight } => {
            lock.stop(&plan.key, &why, in_flight)?;
            failed.map_or(Ok(RunStatus::Stopped(why)), Err)
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
    emit_event(dir, &plan.spec_hash, span, name, level, attrs)
}

/// One event line in `dir`'s log, in the trace of the study with `spec_hash`.
fn emit_event(
    dir: &Path,
    spec_hash: &[u8; 32],
    span: Option<&str>,
    name: &str,
    level: &str,
    attrs: Value,
) -> Result<(), RunError> {
    events::emit(
        &dir.join("events.jsonl"),
        Event {
            ts: crate::time::now_rfc3339(),
            trace_id: events::trace_id(spec_hash),
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
    /// The first event write that failed during the row in flight.
    failed: RefCell<Option<RunError>>,
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

    /// Keep the first event write of the row that fails; the row's records still land.
    fn keep(&self, written: Result<(), RunError>) {
        if let Err(e) = written {
            self.failed.borrow_mut().get_or_insert(e);
        }
    }
}

impl Watch for RunWatch<'_> {
    fn tick(&self) -> bool {
        if self.last_beat.get().elapsed() >= self.plan.heartbeat {
            self.last_beat.set(Instant::now());
            let (index, span) = self.row.borrow().clone();
            let attrs = json!({ "row": index });
            // A failed write shows at the row's end event, which pauses the run.
            let _ = log_event(self.plan, self.dir, Some(&span), "heartbeat", "info", attrs);
        }
        // A failed or busy read is retried on the next tick; a boundary pauses on a failure.
        if let Ok(Some(Signal::Stop(why))) = self.control.peek_now() {
            let in_flight = Some(self.row.borrow().0);
            self.end(Halt::Stop { why, in_flight });
        }
        if sys::interrupted() {
            self.end(Halt::Pause(INTERRUPTED.into()));
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
    unknown_cpu: u64,
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
            t.unknown_cpu += u64::from(r.body.get("cpu_unknown").is_some());
            if r.kind == "row" {
                t.voided += u64::from(r.body["exit"] != "ok");
                t.retried += r.body["attempts"].as_u64().unwrap_or(1).saturating_sub(1);
            }
        }
        t
    }

    /// One attempt; `row` when a row record holds it, as `of` counts.
    fn add(&mut self, cpu: f64, wall: f64, row: bool) {
        self.rows += u64::from(row);
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
    books: &mut Books,
    watch: &RunWatch,
) -> Result<Option<Halt>, RunError> {
    let span = events::new_span_id();
    watch.begin(row.index, &span);
    let start = json!({ "row": row.index });
    log_event(plan, dir, Some(&span), "row.start", "info", start)?;
    let (exit, halt) = attempt_row(plan, row, dir, log, books, watch)?;
    let end = json!({ "row": row.index, "exit": exit });
    watch.keep(log_event(plan, dir, Some(&span), "row.end", "info", end));
    Ok(halt)
}

/// A row's attempts, retries included, then its record. Returns the recorded exit, or
/// `stopped` or `paused` for a row that ends the run with only spend records. The row
/// record holds the last attempt's spend; each earlier attempt has its own spend record.
fn attempt_row(
    plan: &RunPlan,
    row: &Row,
    dir: &Path,
    log: &Path,
    books: &mut Books,
    watch: &RunWatch,
) -> Result<(String, Option<Halt>), RunError> {
    let mut attempts = 0u32;
    let mut unknown = false;
    let mut bound;
    let span = watch.row.borrow().1.clone();
    // An attempt's events come after its record, so a failed event write cannot lose the
    // attempt's spend.
    let noted = |o: &RowOutcome| attempt_events(plan, dir, &span, row.index, o);
    let outcome = loop {
        attempts += 1;
        let job = job_for(plan, row, dir)?;
        let mut o = plan
            .backend
            .run_row(&job, watch)
            .unwrap_or_else(|e| RowOutcome {
                exit: Exit::Infra(e.to_string()),
                cpu_seconds: 0.0,
                wall_seconds: 0.0,
                output_hash: String::new(),
                selftest: None,
                cpu_unknown: None,
                ended: false,
            });
        // A row the box ended, or whose meter cannot read, is charged a bound. A meter that
        // cannot read also pauses the run once the row is recorded.
        (o.cpu_seconds, bound) = charge(&o, plan.backend.cpus(), books.per_row.cpu_seconds);
        unknown |= o.cpu_unknown.is_some();
        control::row_ended(&plan.home, &plan.study_id);
        // A row ended early is not data: its spend is recorded, and the run ends.
        if let Some(h) = watch.ended.borrow_mut().take() {
            unrecorded(plan, log, row, books, &o, bound, &h.reason())?;
            watch.keep(noted(&o));
            return Ok((h.exit().into(), Some(h)));
        }
        // A stop nobody asked for is a backend fault: void the row and pause, no retry.
        if o.exit == Exit::Stopped {
            let exit = Exit::Infra("the backend stopped the row with no stop requested".into());
            break RowOutcome { exit, ..o };
        }
        let why = match &o.exit {
            Exit::Infra(why) if attempts <= MAX_RETRIES => why.clone(),
            _ => break o,
        };
        // The failed attempt is spend first. A retry then needs a healthy backend and room
        // in the budget, as a new row does.
        unrecorded(
            plan,
            log,
            row,
            books,
            &o,
            bound,
            &format!("infrastructure fault: {why}"),
        )?;
        watch.keep(noted(&o));
        let failed = || watch.failed.borrow().is_some();
        let halt = if failed() {
            None
        } else {
            match plan.backend.probe() {
                Probe::Unavailable(why) => Some(Halt::Pause(format!("backend unavailable: {why}"))),
                // The gate fails only on an alert write, held as any failed event write is.
                Probe::Available => gate(plan, dir, books).unwrap_or_else(|e| {
                    watch.keep(Err(e));
                    None
                }),
            }
        };
        // No new attempt once the event log fails: the run pauses after the row, for the
        // attempt's unknown CPU count when it has one.
        let event_log = || Halt::Pause(EVENT_LOG.into());
        let halt = halt.or_else(|| {
            failed().then(|| {
                if unknown {
                    unknown_cpu(row)
                } else {
                    event_log()
                }
            })
        });
        if let Some(h) = halt {
            return Ok((h.exit().into(), Some(h)));
        }
    };
    let (exit, reason) = match &outcome.exit {
        Exit::Ok => ("ok".to_string(), None),
        Exit::Payload(c) => (format!("payload:{c}"), None),
        Exit::Void(why) => ("void:output".to_string(), Some(why)),
        Exit::Infra(_) | Exit::Stopped => ("void:infra".to_string(), None),
    };
    let mut body = json!({
        "condition": row.condition,
        "unit": row.unit,
        "repeat": row.repeat,
        "seed": row.seed_hex,
        "isolation": plan.backend.level(),
        "launcher": plan.backend.launcher(),
        "environment": plan.environment,
        "payload_sha256": plan.payload_sha256,
        "data_hashes": plan.inputs.iter().filter_map(|(_, h)| h.as_ref()).collect::<Vec<_>>(),
        "cpu_seconds": outcome.cpu_seconds,
        "wall_seconds": outcome.wall_seconds,
        "output_hash": outcome.output_hash,
        "exit": exit,
        "attempts": attempts,
    });
    if let Some(why) = reason {
        body["reason"] = json!(why);
    }
    if let Some(why) = &outcome.cpu_unknown {
        body["cpu_unknown"] = json!(why);
    }
    if bound {
        body["cpu_bound"] = json!(true);
    }
    records::append(
        log,
        &plan.key,
        RecordDraft {
            study: plan.study_id.clone(),
            row: Some(row.index),
            kind: "row".into(),
            body,
        },
    )?;
    books.add(plan, &outcome, true);
    watch.keep(noted(&outcome));
    let halt = match outcome.exit {
        Exit::Infra(why) => Some(Halt::Pause(format!(
            "row {} voided by infrastructure faults: {why}",
            row.index
        ))),
        _ => unknown.then(|| unknown_cpu(row)),
    };
    Ok((exit, halt))
}

/// The CPU seconds to charge for an attempt, and whether that figure is a bound. A row the
/// box ended, or whose CPU count is unknown, may have used more than was measured: it is
/// charged the CPU it could have used, wall time times the CPUs it may use. An unknown
/// count is also charged at least the row estimate.
pub fn charge(o: &RowOutcome, cpus: u32, estimate_cpu: f64) -> (f64, bool) {
    let unknown = o.cpu_unknown.is_some();
    if !(o.ended || unknown) {
        return (o.cpu_seconds, false);
    }
    let floor = if unknown { estimate_cpu } else { 0.0 };
    let could_use = o.wall_seconds * f64::from(cpus);
    (o.cpu_seconds.max(could_use).max(floor), true)
}

/// The pause for a row whose CPU count the backend could not read.
fn unknown_cpu(row: &Row) -> Halt {
    Halt::Pause(format!(
        "row {} has an unknown CPU count; it is charged the row estimate",
        row.index
    ))
}

/// The events of one recorded attempt: what the self-test found, and the charge for a CPU
/// count the backend could not read.
fn attempt_events(
    plan: &RunPlan,
    dir: &Path,
    span: &str,
    row: u64,
    o: &RowOutcome,
) -> Result<(), RunError> {
    if let Some(t) = &o.selftest {
        let attrs = json!({
            "row": row,
            "connect_blocked": t.connect_blocked,
            "vsock_blocked": t.vsock_blocked,
            "io_uring_blocked": t.io_uring_blocked,
            "vsock": t.vsock_state(),
            "io_uring": t.io_uring_state(),
        });
        log_event(plan, dir, Some(span), "sandbox.selftest", "info", attrs)?;
    }
    if let Some(reason) = &o.cpu_unknown {
        let attrs = json!({
            "row": row,
            "reason": reason,
            "charged_cpu_seconds": o.cpu_seconds,
        });
        log_event(plan, dir, Some(span), "row.cpu_unknown", "warn", attrs)?;
    }
    Ok(())
}

/// An attempt no row record holds: its spend goes into one signed `spend` record, so a
/// resume counts it, and into this session's books.
fn unrecorded(
    plan: &RunPlan,
    log: &Path,
    row: &Row,
    books: &mut Books,
    o: &RowOutcome,
    bound: bool,
    reason: &str,
) -> Result<(), RunError> {
    let mut body =
        json!({ "cpu_seconds": o.cpu_seconds, "wall_seconds": o.wall_seconds, "reason": reason });
    if let Some(why) = &o.cpu_unknown {
        body["cpu_unknown"] = json!(why);
    }
    if bound {
        body["cpu_bound"] = json!(true);
    }
    records::append(
        log,
        &plan.key,
        RecordDraft {
            study: plan.study_id.clone(),
            row: Some(row.index),
            kind: "spend".into(),
            body,
        },
    )?;
    books.add(plan, o, false);
    Ok(())
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
        ],
        input_dir: plan.view.clone(),
        output_dir: out,
        seed,
        wall_timeout: wall_timeout(plan.spec.design.row_seconds),
        rootfs: plan.rootfs.clone(),
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
