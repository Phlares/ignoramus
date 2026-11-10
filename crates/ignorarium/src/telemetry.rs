//! Host readings that a row record keeps for later study. They are observation only: no
//! decision, hash, charge or comparison reads them. A reading that cannot be had is recorded
//! with its reason, never raised as an error.
use crate::backend::cgroup::CgroupCounters;
use crate::canon::sha256_hex;
use crate::sys;
use serde::{Serialize, Serializer};
use serde_json::{Value, json};
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
