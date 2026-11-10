use crate::value;
use serde_json::Value;
use std::fs;
use std::hint::black_box;
use std::path::{Component, Path};
use std::time::Duration;

/// Runs one row. `unit` is `<path>#<index>`: a path under `in_dir` and a 0-based line index.
/// `params` is the condition's JSON object: `work_ms` and `mem_mib`, each 0 when absent.
/// Writes `out_dir/values.json` last, after every check passed.
pub fn payload(
    condition: &str,
    unit: &str,
    in_dir: &Path,
    out_dir: &Path,
    params: &str,
) -> Result<(), String> {
    let (path, index) = unit.rsplit_once('#').ok_or("unit must be <path>#<index>")?;
    let index: u32 = index.parse().map_err(|e| format!("unit index: {e}"))?;
    let inside = !path.is_empty()
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    if !inside {
        return Err(format!("unit path {path:?} is not under the input folder"));
    }
    let params: Value = serde_json::from_str(params).map_err(|e| format!("params: {e}"))?;
    if !params.is_object() {
        return Err("params must be a JSON object".into());
    }
    let work_ms = param(&params, "work_ms")?;
    let mem_mib = param(&params, "mem_mib")?;

    let bytes = fs::read(in_dir.join(path)).map_err(|e| format!("{path}: {e}"))?;
    let units = String::from_utf8_lossy(&bytes)
        .lines()
        .filter(|l| !l.is_empty())
        .count();
    if index as usize >= units {
        return Err(format!(
            "{path} holds {units} units; index {index} is past the end"
        ));
    }
    let y = value(&bytes, index, condition).ok_or(format!("unknown condition {condition:?}"))?;

    let _held = touch(mem_mib)?;
    burn(work_ms);
    let text = serde_json::json!({ "y": y }).to_string();
    let tmp = out_dir.join("values.json.tmp");
    fs::write(&tmp, text)
        .and_then(|()| fs::rename(&tmp, out_dir.join("values.json")))
        .map_err(|e| {
            let _ = fs::remove_file(&tmp);
            format!("values.json: {e}")
        })
}

fn param(params: &Value, key: &str) -> Result<u64, String> {
    params.get(key).map_or(Ok(0), |v| {
        v.as_u64()
            .ok_or(format!("{key} must be a non-negative integer"))
    })
}

/// Spins until this process has used `ms` milliseconds of CPU time.
fn burn(ms: u64) {
    let end = cpu_time() + Duration::from_millis(ms);
    while cpu_time() < end {
        black_box(());
    }
}

/// The CPU time this process has used. Off Unix, the time since the first call.
#[cfg(unix)]
fn cpu_time() -> Duration {
    let ts = rustix::time::clock_gettime(rustix::time::ClockId::ProcessCPUTime);
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

#[cfg(not(unix))]
fn cpu_time() -> Duration {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed()
}

/// `mib` MiB of written memory. The caller holds it until the process exits.
fn touch(mib: u64) -> Result<Vec<u8>, String> {
    let len = usize::try_from(mib)
        .ok()
        .and_then(|m| m.checked_mul(1 << 20))
        .ok_or("mem_mib is too large")?;
    let mut mem = Vec::new();
    mem.try_reserve_exact(len)
        .map_err(|_| "cannot allocate mem_mib")?;
    mem.resize(len, 1);
    Ok(black_box(mem))
}
