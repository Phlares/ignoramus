//! The machine profile: what this box can spend. Read from `<home>/machine.toml`.
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Deserialize)]
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
    toml::from_str(&text).map_err(|e| MachineError {
        file,
        msg: e.to_string(),
    })
}
