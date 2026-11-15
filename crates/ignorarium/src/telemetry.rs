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
    /// Each `cpu_stat.<line>` (microseconds) and `memory_events.<line>`, summed.
    pub cgroup: Summed<BTreeMap<String, f64>>,
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
            if let Some(c) = reading(&mut t.cpu_ticks, telemetry, "cpu_ticks") {
                let counts =
                    |k: &str| (c[k].as_array().into_iter().flatten()).filter_map(Value::as_u64);
                let pairs = counts("start").zip(counts("end"));
                if let Some(hz) = c["user_hz"].as_u64().filter(|h| *h > 0) {
                    for (name, (s, e)) in CPU_TICK_FIELDS.iter().zip(pairs) {
                        let seconds = t.cpu_ticks.value.entry(name.to_string()).or_default();
                        *seconds += e.saturating_sub(s) as f64 / hz as f64;
                    }
                }
            }
            if let Some(g) = reading(&mut t.cgroup, telemetry, "cgroup") {
                for part in ["cpu_stat", "memory_events"] {
                    for (k, v) in g[part].as_object().into_iter().flatten() {
                        let total = t.cgroup.value.entry(format!("{part}.{k}")).or_default();
                        *total += v.as_f64().unwrap_or_default();
                    }
                }
                t.memory_peak_max = t.memory_peak_max.max(g["memory_peak"].as_u64());
            }
        }
        t.host_cpu.value = models.into_iter().collect();
        t
    }
}

/// The four fields of a row record that the overview reads.
#[derive(Debug, Clone, PartialEq)]
pub struct RowView {
    pub row: u64,
    pub wall_seconds: Option<f64>,
    pub cpu_seconds: Option<f64>,
    pub telemetry: Option<Value>,
}

impl RowView {
    /// The view of a row record; `None` for any other record.
    pub fn of(r: &crate::records::Record) -> Option<RowView> {
        if r.kind != "row" {
            return None;
        }
        Some(RowView {
            row: r.row?,
            wall_seconds: r.body["wall_seconds"].as_f64(),
            cpu_seconds: r.body["cpu_seconds"].as_f64(),
            telemetry: r.body.get("telemetry").cloned(),
        })
    }

    /// Tick deltas of the row, one per `CPU_TICK_FIELDS` field the record holds, and `user_hz`.
    fn deltas(&self) -> Option<(f64, Vec<u64>)> {
        let c = self.telemetry.as_ref()?.get("cpu_ticks")?;
        let hz = c["user_hz"].as_u64().filter(|h| *h > 0)?;
        let counts = |k: &str| (c[k].as_array().into_iter().flatten()).filter_map(Value::as_u64);
        let d = counts("start").zip(counts("end"));
        Some((hz as f64, d.map(|(s, e)| e.saturating_sub(s)).collect()))
    }

    /// Seconds of the `CPU_TICK_FIELDS` field `name`, end minus start.
    fn tick_seconds(&self, name: &str) -> Option<f64> {
        let (hz, d) = self.deltas()?;
        Some(*d.get(tick_index(name))? as f64 / hz)
    }

    /// The delta of `name` over the sum of every field's delta; `None` when no tick passed.
    fn tick_share(&self, name: &str) -> Option<f64> {
        let (_, d) = self.deltas()?;
        let total: u64 = d.iter().sum();
        let own = *d.get(tick_index(name))?;
        (total > 0).then(|| own as f64 / total as f64)
    }
}

fn tick_index(name: &str) -> usize {
    CPU_TICK_FIELDS.iter().position(|f| *f == name).unwrap_or(0)
}

/// Largest and mean of `v` as JSON numbers; null when `v` is empty.
fn spread(v: &[f64]) -> (Value, Value) {
    if v.is_empty() {
        return (Value::Null, Value::Null);
    }
    let max = v.iter().copied().fold(f64::MIN, f64::max);
    (json!(max), json!(v.iter().sum::<f64>() / v.len() as f64))
}

/// `{rows, max_s, mean_s}` of `v`.
fn seconds(v: &[f64]) -> Value {
    let (max_s, mean_s) = spread(v);
    json!({ "rows": v.len(), "max_s": max_s, "mean_s": mean_s })
}

/// A report on the telemetry of `rows`, in blocks of `order`. A block is one of ten
/// consecutive parts of `order` whose sizes differ by at most one, so its edges depend on
/// `order` alone. `blind` leaves wall and cpu seconds out of the blocks. The first view of a
/// row index counts.
pub fn overview(rows: &[RowView], order: &[u64], blind: bool) -> Value {
    let mut seen = BTreeSet::new();
    let rows: Vec<&RowView> = rows.iter().filter(|r| seen.insert(r.row)).collect();
    let telemetry = |r: &RowView, k: &str| r.telemetry.as_ref().and_then(|t| t.get(k)).cloned();

    let mut cgroup: BTreeMap<String, u64> = BTreeMap::new();
    let mut cpus: BTreeMap<(String, String), u64> = BTreeMap::new();
    let mut ticks_unavailable = 0;
    for r in &rows {
        if let Some(g) = telemetry(r, "cgroup").filter(Value::is_object) {
            let key = match g.get("unavailable") {
                None => "available".to_string(),
                Some(w) => w.as_str().unwrap_or_default().to_string(),
            };
            *cgroup.entry(key).or_default() += 1;
        }
        if let Some(h) = telemetry(r, "host_cpu").filter(|h| h.get("unavailable").is_none()) {
            let text = |k: &str| h[k].as_str().unwrap_or_default().to_string();
            *cpus
                .entry((text("model"), text("flags_sha256")))
                .or_default() += 1;
        }
        if telemetry(r, "cpu_ticks").is_some() && r.deltas().is_none() {
            ticks_unavailable += 1;
        }
    }

    let secs =
        |name: &str| -> Vec<f64> { rows.iter().filter_map(|r| r.tick_seconds(name)).collect() };
    let all = |f: fn(&RowView) -> Option<f64>, rs: &[&RowView]| -> Vec<f64> {
        rs.iter().filter_map(|r| f(r)).collect()
    };
    let wall = |r: &RowView| r.wall_seconds;
    let cpu = |r: &RowView| r.cpu_seconds;

    let k = order.len().min(10);
    let mut blocks = Vec::new();
    let mut at = 0;
    for b in 0..k {
        let size = order.len() / k + usize::from(b < order.len() % k);
        let part = &order[at..at + size];
        at += size;
        let held: Vec<&RowView> = rows
            .iter()
            .copied()
            .filter(|r| part.contains(&r.row))
            .collect();
        let shares = |name: &str| -> (Value, Value) {
            spread(
                &held
                    .iter()
                    .filter_map(|r| r.tick_share(name))
                    .collect::<Vec<_>>(),
            )
        };
        let (steal_max, steal_mean) = shares("steal");
        let (io_max, io_mean) = shares("iowait");
        let mut block = json!({
            "first_row": part[0],
            "last_row": part[size - 1],
            "rows": held.len(),
            "steal_share_max": steal_max,
            "steal_share_mean": steal_mean,
            "iowait_share_max": io_max,
            "iowait_share_mean": io_mean,
        });
        if !blind {
            let (wall_max, wall_mean) = spread(&all(wall, &held));
            let (cpu_max, cpu_mean) = spread(&all(cpu, &held));
            block["wall_max_s"] = wall_max;
            block["wall_mean_s"] = wall_mean;
            block["cpu_max_s"] = cpu_max;
            block["cpu_mean_s"] = cpu_mean;
        }
        blocks.push(block);
    }

    let (wall_max, wall_mean) = spread(&all(wall, &rows));
    let (cpu_max, cpu_mean) = spread(&all(cpu, &rows));
    json!({
        "rows": rows.len(),
        "cgroup": cgroup,
        "host_cpu": cpus
            .into_iter()
            .map(|((model, flags_sha256), n)| json!({"model": model, "flags_sha256": flags_sha256, "rows": n}))
            .collect::<Vec<_>>(),
        "steal": seconds(&secs("steal")),
        "iowait": seconds(&secs("iowait")),
        "ticks_unavailable": ticks_unavailable,
        "wall": {"max_s": wall_max, "mean_s": wall_mean},
        "cpu": {"max_s": cpu_max, "mean_s": cpu_mean},
        "blocks": blocks,
    })
}
