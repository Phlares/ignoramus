//! The machine profile: what this box can spend. Read from `<home>/machine.toml`.
use crate::backend::{Backend, Probe};
use ignorarium_guest::agent::steps::meminfo_kb;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Machine {
    pub name: String,
    pub cpus: u32,
    pub mem_gib: u32,
    pub max_level: u8,
    pub rate_usd_per_hour: f64,
    pub experiment_mode: bool,
    pub long_job_minutes: u32,
}

impl Default for Machine {
    fn default() -> Self {
        Machine {
            name: "local".into(),
            cpus: 1,
            mem_gib: 1,
            max_level: 0,
            rate_usd_per_hour: 0.0,
            experiment_mode: false,
            long_job_minutes: 30,
        }
    }
}

#[derive(Debug)]
pub struct MachineError {
    pub file: PathBuf,
    pub msg: String,
}

impl std::fmt::Display for MachineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.file.display(), self.msg)
    }
}

impl std::error::Error for MachineError {}

/// An absent file gives the defaults. A bad file is an error, never the defaults.
pub fn load(home: &Path) -> Result<Machine, MachineError> {
    let file = home.join("machine.toml");
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Machine::default()),
        Err(e) => {
            return Err(MachineError {
                file,
                msg: e.to_string(),
            });
        }
    };
    let m: Machine = toml::from_str(&text).map_err(|e| MachineError {
        file: file.clone(),
        msg: e.to_string(),
    })?;
    let bad = |field: &str, need: &str| MachineError {
        file: file.clone(),
        msg: format!("{field} must be {need}"),
    };
    if !m.rate_usd_per_hour.is_finite() || m.rate_usd_per_hour < 0.0 {
        return Err(bad("rate_usd_per_hour", "a finite number, 0 or more"));
    }
    if m.cpus == 0 {
        return Err(bad("cpus", "at least 1"));
    }
    if m.long_job_minutes == 0 {
        return Err(bad("long_job_minutes", "at least 1"));
    }
    Ok(m)
}

/// Ask each backend whether it can run here, as (level, answer).
pub fn probe_backends(backends: &[Box<dyn Backend>]) -> Vec<(u8, Probe)> {
    backends.iter().map(|b| (b.level(), b.probe())).collect()
}

/// Fill a profile from this host and the backends' answers. `rate_usd_per_hour`,
/// `experiment_mode` and `long_job_minutes` come from an existing `machine.toml`. A file that
/// fails to load is an error; the probe never writes.
pub fn measure(home: &Path, probes: &[(u8, Probe)]) -> Result<Machine, MachineError> {
    let kept = load(home)?;
    let name = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "local".into());
    let cpus = std::thread::available_parallelism()
        .map(|n| u32::try_from(n.get()).unwrap_or(u32::MAX))
        .unwrap_or(1);
    let mem_gib = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| mem_gib_of(&t))
        .unwrap_or(1);
    let max_level = probes
        .iter()
        .filter(|(_, p)| *p == Probe::Available)
        .map(|(level, _)| *level)
        .max()
        .unwrap_or(0);
    Ok(Machine {
        name,
        cpus,
        mem_gib,
        max_level,
        ..kept
    })
}

/// Whole GiB (at least 1) from the `MemTotal` line of `/proc/meminfo` text.
pub fn mem_gib_of(meminfo: &str) -> Option<u32> {
    let kib = meminfo_kb(meminfo, "MemTotal")?;
    Some(u32::try_from((kib / (1024 * 1024)).max(1)).unwrap_or(u32::MAX))
}

/// Write `<home>/machine.toml` atomically.
pub fn write(home: &Path, m: &Machine) -> Result<(), MachineError> {
    let file = home.join("machine.toml");
    let err = |msg: String| MachineError {
        file: file.clone(),
        msg,
    };
    let text = toml::to_string(m).map_err(|e| err(e.to_string()))?;
    crate::files::replace_atomic(&file, text.as_bytes(), 0o600).map_err(|e| err(e.to_string()))
}
