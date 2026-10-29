//! The run engine: writes the schedule, runs each row on a backend, records each row.
//! A row is done when its signed record exists. A crash before the record reruns the row.
use crate::backend::{self, Backend, Exit, Probe, RowJob, RowOutcome};
use crate::budget::{Alerts, Cost, CostMeter, Decision, HostMeter, decide, dimension_name};
use crate::canon::sha256_hex;
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
use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Infrastructure failures retried per row, after a health check each.
pub const MAX_RETRIES: u32 = 3;
const MIN_TIMEOUT: Duration = Duration::from_secs(60);
const TIMEOUT_FACTOR: f64 = 10.0;
const PATH_ENV: &str = "/usr/bin:/bin";

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    Done,
    Paused(String),
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

/// Run every row that has no record. Before each row the meter is read; the run pauses
/// instead of starting a row when a cap is reached or the row would reach one.
pub fn run(plan: &RunPlan) -> Result<RunStatus, RunError> {
    let dir = plan.home.study_dir(&plan.study_id);
    let log = dir.join("records.jsonl");
    let machine = machine::load(&plan.home.root).map_err(io)?;
    let all = records::read_all(&log, &plan.key.verifying_key())?;
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
        let sampled = plan.meter.sample();
        let now = sampled.clone().unwrap_or_default();
        if let Ok(c) = &sampled {
            for (dim, fraction) in alerts.fresh(c, budget) {
                let attrs = json!({ "dimension": dimension_name(dim), "fraction": fraction });
                log_event(plan, &dir, "budget.alert", attrs)?;
            }
        }
        let next = per_row.larger(&tally.mean(machine.rate_usd_per_hour));
        if let Decision::Pause(why) = decide(&now, &next, budget, sampled.map(|_| ())) {
            return pause(plan, &dir, why.to_string());
        }
        if let Some(why) = run_row(plan, row, &dir, &log, &mut tally)? {
            return pause(plan, &dir, why);
        }
    }
    Ok(RunStatus::Done)
}

fn pause(plan: &RunPlan, dir: &Path, reason: String) -> Result<RunStatus, RunError> {
    log_event(plan, dir, "run.pause", json!({ "reason": reason }))?;
    Ok(RunStatus::Paused(reason))
}

/// One event line in the study's event log. The study is the trace.
fn log_event(plan: &RunPlan, dir: &Path, name: &str, attrs: Value) -> Result<(), RunError> {
    events::emit(
        &dir.join("events.jsonl"),
        Event {
            ts: crate::time::now_rfc3339(),
            trace_id: hex::encode(&plan.spec_hash[..16]),
            span_id: None,
            name: name.into(),
            level: "warn".into(),
            attrs,
        },
    )
    .map_err(io)
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

/// One row, retries included. `Some(reason)` means the run must pause.
fn run_row(
    plan: &RunPlan,
    row: &Row,
    dir: &Path,
    log: &Path,
    tally: &mut Tally,
) -> Result<Option<String>, RunError> {
    let mut attempts = 0u32;
    let (mut cpu, mut wall) = (0.0, 0.0);
    let outcome = loop {
        attempts += 1;
        let job = job_for(plan, row, dir)?;
        let o = plan.backend.run_row(&job).unwrap_or_else(|e| RowOutcome {
            exit: Exit::Infra(e.to_string()),
            cpu_seconds: 0.0,
            wall_seconds: 0.0,
            output_hash: String::new(),
            selftest_ok: false,
        });
        cpu += o.cpu_seconds;
        wall += o.wall_seconds;
        if !matches!(o.exit, Exit::Infra(_)) || attempts > MAX_RETRIES {
            break o;
        }
        if let Probe::Unavailable(why) = plan.backend.probe() {
            return Ok(Some(format!("backend unavailable: {why}")));
        }
    };
    let exit = match &outcome.exit {
        Exit::Ok => "ok".to_string(),
        Exit::Payload(c) => format!("payload:{c}"),
        Exit::Infra(_) => "void:infra".to_string(),
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
    Ok(match outcome.exit {
        Exit::Infra(why) => Some(format!(
            "row {} voided by infrastructure faults: {why}",
            row.index
        )),
        _ => None,
    })
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
