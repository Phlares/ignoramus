//! Host readings that a row record keeps for later study. They are observation only: no
//! decision, hash, charge or comparison reads them. A reading that cannot be had is recorded
//! with its reason, never raised as an error.
use crate::backend::cgroup::CgroupCounters;
use crate::canon::sha256_hex;
use crate::sys;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// A value read from the host, or the reason it could not be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading<T> {
    Read(T),
    Unavailable(String),
}

impl<T: Serialize> Serialize for Reading<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Read(v) => v.serialize(s),
            Self::Unavailable(why) => json!({ "unavailable": why }).serialize(s),
        }
    }
}

impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for Reading<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let v = Value::deserialize(d)?;
        if let Some(why) = v.get("unavailable").and_then(Value::as_str) {
            return Ok(Self::Unavailable(why.into()));
        }
        serde_json::from_value(v)
            .map(Self::Read)
            .map_err(D::Error::custom)
    }
}

impl<T> Reading<T> {
    fn unavailable(why: &str) -> Self {
        Self::Unavailable(why.into())
    }
}

/// The first CPU of `/proc/cpuinfo`: its model name and the SHA-256 of its flags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostCpu {
    pub model: String,
    pub flags_sha256: String,
}

/// The value of the first `key` line of a `/proc` text, whose lines read `key : value`.
fn first_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    (text.lines())
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim())
}

/// The CPU model and flags hash in `cpuinfo`. An aarch64 text names neither field.
pub fn host_cpu_of(cpuinfo: &str) -> Reading<HostCpu> {
    let Some(model) = first_value(cpuinfo, "model name") else {
        return Reading::unavailable("/proc/cpuinfo names no model name");
    };
    let Some(flags) = first_value(cpuinfo, "flags") else {
        return Reading::unavailable("/proc/cpuinfo names no flags");
    };
    Reading::Read(HostCpu {
        model: model.into(),
        flags_sha256: sha256_hex(flags.as_bytes()),
    })
}

/// The host CPU, read once per process. A resumed run is a new process and reads it again.
pub fn host_cpu() -> &'static Reading<HostCpu> {
    static CPU: OnceLock<Reading<HostCpu>> = OnceLock::new();
    CPU.get_or_init(|| match std::fs::read_to_string("/proc/cpuinfo") {
        Ok(text) => host_cpu_of(&text),
        Err(e) => Reading::Unavailable(format!("/proc/cpuinfo: {e}")),
    })
}

/// The aggregate `cpu ` line of `/proc/stat`, raw, in kernel order: user nice system idle
/// iowait irq softirq steal guest guest_nice. An old kernel gives fewer fields. The counts
/// are in `1 / user_hz` seconds and cover the whole host, so concurrent runs share them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuTicks {
    pub user_hz: u64,
    pub ticks: Vec<u64>,
}

/// The aggregate CPU line of `stat`, a `/proc/stat` text.
pub fn cpu_ticks_of(stat: &str, user_hz: u64) -> Reading<CpuTicks> {
    let Some(fields) = (stat.lines())
        .map(|l| l.split_whitespace().collect::<Vec<_>>())
        .find(|f| f.first() == Some(&"cpu"))
    else {
        return Reading::unavailable("/proc/stat holds no cpu line");
    };
    let ticks: Result<Vec<u64>, _> = fields[1..].iter().map(|f| f.parse()).collect();
    match ticks {
        Ok(ticks) if !ticks.is_empty() => Reading::Read(CpuTicks { user_hz, ticks }),
        _ => Reading::unavailable("/proc/stat cpu line holds no counts or a bad one"),
    }
}

/// The host CPU counts now.
pub fn cpu_ticks() -> Reading<CpuTicks> {
    let Some(hz) = sys::clock_ticks() else {
        return Reading::unavailable("the clock tick rate is unknown");
    };
    match std::fs::read_to_string("/proc/stat") {
        Ok(text) => cpu_ticks_of(&text, hz),
        Err(e) => Reading::Unavailable(format!("/proc/stat: {e}")),
    }
}

/// The `telemetry` key of a row record. `why` rewrites every reason, as for any text that
/// reaches the records. Two tick reads that disagree on a length are kept as they are;
/// the reader compares.
pub fn record(
    host: &Reading<HostCpu>,
    start: &Reading<CpuTicks>,
    end: &Reading<CpuTicks>,
    cgroup: &Reading<CgroupCounters>,
    why: &dyn Fn(&str) -> String,
) -> Value {
    let ticks = match (start, end) {
        (Reading::Read(s), Reading::Read(e)) => {
            Reading::Read(json!({ "user_hz": s.user_hz, "start": s.ticks, "end": e.ticks }))
        }
        (Reading::Unavailable(w), _) | (_, Reading::Unavailable(w)) => {
            Reading::Unavailable(w.clone())
        }
    };
    json!({
        "host_cpu": shown(host, why),
        "cpu_ticks": shown(&ticks, why),
        "cgroup": shown(cgroup, why),
    })
}

/// `r` as its value, or as its reason rewritten by `why`.
fn shown<T: Serialize>(r: &Reading<T>, why: &dyn Fn(&str) -> String) -> Value {
    match r {
        Reading::Read(v) => json!(v),
        Reading::Unavailable(w) => json!({ "unavailable": why(w) }),
    }
}

/// The kernel's names of the `cpu ` line fields of `/proc/stat`, in order.
pub const CPU_TICK_FIELDS: [&str; 10] = [
    "user",
    "nice",
    "system",
    "idle",
    "iowait",
    "irq",
    "softirq",
    "steal",
    "guest",
    "guest_nice",
];

/// One reading over many row records: how many held it, how many held its reason instead,
/// and what the read ones give together.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Summed<T> {
    pub read: u64,
    pub unavailable: u64,
    pub value: T,
}

/// The `telemetry` keys of row records, added up. A reason never counts as zero.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Totals {
    /// Row records, and those that hold a `telemetry` key.
    pub rows: u64,
    pub recorded: u64,
    /// Each host CPU model the rows name.
    pub host_cpu: Summed<Vec<String>>,
    /// Seconds of each `CPU_TICK_FIELDS` field, end minus start. The counts are host-wide, so
    /// rows that ran at the same time count the same seconds.
    pub cpu_ticks: Summed<BTreeMap<String, f64>>,
    /// The cgroup figures of rows below level 2, under the keys cards have always used.
    #[serde(flatten)]
    pub process: CgroupTotals,
    /// The cgroup figures of level 2 rows: the VMM with the guest it runs. `None` when no row
    /// ran at level 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<CgroupTotals>,
}

/// Rows below level 2 run their processes in the row cgroup; level 2 rows run a machine
/// there, so their figures never add to the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LevelClass {
    Process,
    Machine,
}

impl LevelClass {
    /// The class of a row record's `isolation`; a record without one predates level 2.
    pub fn of(isolation: Option<u64>) -> LevelClass {
        match isolation {
            Some(2..) => LevelClass::Machine,
            _ => LevelClass::Process,
        }
    }
}

/// The cgroup readings of one level class, added up.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CgroupTotals {
    /// Each `cpu_stat.<line>` (microseconds) and `memory_events.<line>`, summed.
    #[serde(rename = "cgroup")]
    pub summed: Summed<BTreeMap<String, f64>>,
    /// The largest `memory_peak`, in bytes.
    pub memory_peak_max: Option<u64>,
}

/// `key` of `telemetry`, counted in `into`; `None` when it is absent or a reason.
fn reading<'v, T>(into: &mut Summed<T>, telemetry: &'v Value, key: &str) -> Option<&'v Value> {
    let v = telemetry.get(key)?;
    if v.get("unavailable").is_some() {
        into.unavailable += 1;
        return None;
    }
    into.read += 1;
    Some(v)
}

impl Totals {
    /// The totals over the row record bodies `rows`.
    pub fn of<'a>(rows: impl IntoIterator<Item = &'a Value>) -> Totals {
        let mut t = Totals::default();
        let mut models = BTreeSet::new();
        for body in rows {
            t.rows += 1;
            let Some(telemetry) = body.get("telemetry") else {
                continue;
            };
            t.recorded += 1;
            if let Some(h) = reading(&mut t.host_cpu, telemetry, "host_cpu") {
                models.extend(h["model"].as_str().map(str::to_string));
            }
            if let Some(c) = telemetry.get("cpu_ticks") {
                if let Some((hz, deltas)) = tick_deltas(c) {
                    t.cpu_ticks.read += 1;
                    for (name, n) in CPU_TICK_FIELDS.iter().zip(deltas) {
                        *t.cpu_ticks.value.entry(name.to_string()).or_default() += n as f64 / hz;
                    }
                } else {
                    t.cpu_ticks.unavailable += 1;
                }
            }
            let into = match LevelClass::of(body["isolation"].as_u64()) {
                LevelClass::Process => &mut t.process,
                LevelClass::Machine => t.machine.get_or_insert_default(),
            };
            if let Some(g) = reading(&mut into.summed, telemetry, "cgroup") {
                for part in ["cpu_stat", "memory_events"] {
                    for (k, v) in g[part].as_object().into_iter().flatten() {
                        let total = into.summed.value.entry(format!("{part}.{k}")).or_default();
                        *total += v.as_f64().unwrap_or_default();
                    }
                }
                into.memory_peak_max = into.memory_peak_max.max(g["memory_peak"].as_u64());
            }
        }
        t.host_cpu.value = models.into_iter().collect();
        t
    }
}

/// What a row record holds for one reading.
#[derive(Debug, Clone, PartialEq)]
pub enum Seen<T> {
    /// The key is absent: a record written before the reading existed.
    NotRecorded,
    /// The record holds a reason, or a reading that cannot be used.
    Unavailable(String),
    Read(T),
}

impl<T> Seen<T> {
    /// The reading `key` of `telemetry`, parsed by `parse`.
    fn of(telemetry: &Value, key: &str, parse: impl Fn(&Value) -> Option<T>) -> Self {
        let Some(v) = telemetry.get(key) else {
            return Seen::NotRecorded;
        };
        match v.get("unavailable") {
            Some(why) => Seen::Unavailable(why.as_str().unwrap_or("unavailable").into()),
            None => parse(v).map_or_else(|| Seen::Unavailable("unreadable".into()), Seen::Read),
        }
    }

    /// The value, or one more in `missing` for the state that holds none.
    fn counted(&self, missing: &mut Missing) -> Option<&T> {
        match self {
            Seen::Read(v) => Some(v),
            Seen::Unavailable(_) => {
                missing.unavailable += 1;
                None
            }
            Seen::NotRecorded => {
                missing.not_recorded += 1;
                None
            }
        }
    }
}

/// Rows whose reading is a reason, and rows that never recorded it.
#[derive(Default)]
struct Missing {
    unavailable: u64,
    not_recorded: u64,
}

impl Missing {
    fn json(&self) -> Value {
        json!({ "unavailable": self.unavailable, "not_recorded": self.not_recorded })
    }
}

/// The fields of a row record that the overview reads, each parsed once.
#[derive(Debug, Clone, PartialEq)]
pub struct RowView {
    pub row: u64,
    pub wall_seconds: Option<f64>,
    pub cpu_seconds: Option<f64>,
    /// `user_hz` and the tick deltas, one per `CPU_TICK_FIELDS` field the record holds.
    pub ticks: Seen<(f64, Vec<u64>)>,
    /// The CPU model and the hash of its flags.
    pub host_cpu: Seen<(String, String)>,
    /// `Read` when the row's cgroup counters were read.
    pub cgroup: Seen<()>,
    pub class: LevelClass,
}

/// `user_hz` and the per-field tick deltas of a `cpu_ticks` reading. `None` when the rate is
/// zero or the reads hold too few fields for steal and iowait (an old kernel).
pub fn tick_deltas(c: &Value) -> Option<(f64, Vec<u64>)> {
    let hz = c["user_hz"].as_u64().filter(|h| *h > 0)?;
    let counts = |k: &str| (c[k].as_array().into_iter().flatten()).filter_map(Value::as_u64);
    let d: Vec<u64> = (counts("start").zip(counts("end")))
        .map(|(s, e)| e.saturating_sub(s))
        .collect();
    (d.len() > tick_index("steal").max(tick_index("iowait"))).then_some((hz as f64, d))
}

impl RowView {
    /// The view of a row record; `None` for any other record.
    pub fn of(r: &crate::records::Record) -> Option<RowView> {
        if r.kind != "row" {
            return None;
        }
        let t = r.body.get("telemetry").unwrap_or(&Value::Null);
        let text = |h: &Value, k: &str| h[k].as_str().unwrap_or_default().to_string();
        Some(RowView {
            row: r.row?,
            wall_seconds: r.body["wall_seconds"].as_f64(),
            cpu_seconds: r.body["cpu_seconds"].as_f64(),
            ticks: Seen::of(t, "cpu_ticks", tick_deltas),
            host_cpu: Seen::of(t, "host_cpu", |h| {
                Some((text(h, "model"), text(h, "flags_sha256")))
            }),
            cgroup: Seen::of(t, "cgroup", |g| g.is_object().then_some(())),
            class: LevelClass::of(r.body["isolation"].as_u64()),
        })
    }

    /// Seconds of the `CPU_TICK_FIELDS` field `name`, end minus start.
    fn tick_seconds(&self, name: &str) -> Option<f64> {
        let Seen::Read((hz, d)) = &self.ticks else {
            return None;
        };
        Some(*d.get(tick_index(name))? as f64 / hz)
    }

    /// The delta of `name` over the deltas of user through steal; `None` when no tick passed.
    /// The kernel counts guest time inside user, and guest_nice inside nice, so those two
    /// fields stay out of the sum.
    fn tick_share(&self, name: &str) -> Option<f64> {
        let Seen::Read((_, d)) = &self.ticks else {
            return None;
        };
        let total: u64 = d[..=tick_index("steal")].iter().sum();
        let own = *d.get(tick_index(name))?;
        (total > 0).then(|| own as f64 / total as f64)
    }
}

fn tick_index(name: &str) -> usize {
    (CPU_TICK_FIELDS.iter().position(|f| *f == name)).expect("a CPU_TICK_FIELDS name")
}

/// `{rows, max, mean}` of `v`; `max` and `mean` are null when `v` is empty.
fn spread(v: &[f64]) -> Value {
    if v.is_empty() {
        return json!({ "rows": 0, "max": null, "mean": null });
    }
    let max = v.iter().copied().fold(f64::MIN, f64::max);
    json!({ "rows": v.len(), "max": max, "mean": v.iter().sum::<f64>() / v.len() as f64 })
}

/// The figure `f` of each of `rows` that holds one.
fn times(rows: &[&RowView], f: fn(&RowView) -> Option<f64>) -> Vec<f64> {
    rows.iter().filter_map(|r| f(r)).collect()
}

/// A report on the telemetry of `rows`, in blocks of `order`. A block is one of ten
/// consecutive parts of `order` whose sizes differ by at most one, so its edges depend on
/// `order` alone. `blind` leaves wall and cpu seconds out of the blocks. The first view of a
/// row index counts. Every count of rows adds up to `rows`: a reading a row lacks counts as
/// unavailable or as not recorded.
pub fn overview(rows: &[RowView], order: &[u64], blind: bool) -> Value {
    let mut seen = BTreeSet::new();
    let rows: Vec<&RowView> = rows.iter().filter(|r| seen.insert(r.row)).collect();

    let mut cgroup: BTreeMap<LevelClass, BTreeMap<String, u64>> = BTreeMap::new();
    let mut cpus: BTreeMap<(String, String), u64> = BTreeMap::new();
    let (mut no_cpu, mut no_ticks) = (Missing::default(), Missing::default());
    for r in &rows {
        let state = match &r.cgroup {
            Seen::Read(()) => "available".to_string(),
            Seen::Unavailable(why) => why.clone(),
            Seen::NotRecorded => "not recorded".to_string(),
        };
        *cgroup.entry(r.class).or_default().entry(state).or_default() += 1;
        if let Some(cpu) = r.host_cpu.counted(&mut no_cpu) {
            *cpus.entry(cpu.clone()).or_default() += 1;
        }
        r.ticks.counted(&mut no_ticks);
    }

    let secs =
        |name: &str| -> Vec<f64> { rows.iter().filter_map(|r| r.tick_seconds(name)).collect() };
    let wall = |rs: &[&RowView]| spread(&times(rs, |r| r.wall_seconds));
    let cpu = |rs: &[&RowView]| spread(&times(rs, |r| r.cpu_seconds));

    let k = order.len().min(10);
    let mut parts = Vec::new();
    let mut block_of = std::collections::HashMap::new();
    let mut at = 0;
    for b in 0..k {
        let size = order.len() / k + usize::from(b < order.len() % k);
        let part = &order[at..at + size];
        at += size;
        block_of.extend(part.iter().map(|i| (*i, b)));
        parts.push(part);
    }
    let mut held_by: Vec<Vec<&RowView>> = vec![Vec::new(); k];
    for r in &rows {
        if let Some(b) = block_of.get(&r.row) {
            held_by[*b].push(r);
        }
    }
    let mut blocks = Vec::new();
    for (part, held) in parts.into_iter().zip(held_by) {
        let shares = |name: &str| {
            spread(&(held.iter().filter_map(|r| r.tick_share(name))).collect::<Vec<_>>())
        };
        let mut block = json!({
            "first_row": part[0],
            "last_row": part[part.len() - 1],
            "rows": held.len(),
            "steal_share": shares("steal"),
            "iowait_share": shares("iowait"),
        });
        if !blind {
            block["wall_s"] = wall(&held);
            block["cpu_s"] = cpu(&held);
        }
        blocks.push(block);
    }

    let machine = cgroup.remove(&LevelClass::Machine);
    let mut out = json!({
        "rows": rows.len(),
        "cgroup": cgroup.remove(&LevelClass::Process).unwrap_or_default(),
        "host_cpu": cpus
            .into_iter()
            .map(|((model, flags_sha256), n)| json!({"model": model, "flags_sha256": flags_sha256, "rows": n}))
            .collect::<Vec<_>>(),
        "host_cpu_missing": no_cpu.json(),
        "steal_s": spread(&secs("steal")),
        "iowait_s": spread(&secs("iowait")),
        "ticks_missing": no_ticks.json(),
        "wall_s": wall(&rows),
        "cpu_s": cpu(&rows),
        "blocks": blocks,
    });
    if let Some(m) = machine {
        out["cgroup_machine"] = json!(m);
    }
    out
}
