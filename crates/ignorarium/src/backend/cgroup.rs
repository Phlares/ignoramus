//! A cgroup per row at levels 0 and 1, read once the row is reaped: its peak memory, its
//! memory events and its CPU time. Observation only: the reading never changes a row's exit,
//! output, charge or any decision, and a cgroup that cannot be had is a reason, not a fault.
//!
//! The operator starts the box alone in a delegated cgroup C, such as a
//! `systemd-run --user --scope -p Delegate=yes` scope. The box moves itself into
//! `C/ignorarium-box`, turns on the `memory` controller for C's children, and gives each row a
//! leaf `C/row-<pid>-<n>`. A row's first process enters its leaf before it execs. The box
//! enables no other controller, so it limits nothing; `cpu.stat` times need no controller.
use crate::sys;
use crate::telemetry::Reading;
use serde::Serialize;
use std::collections::BTreeMap;
use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// The reason a row records when its first process never entered its leaf.
pub const MISSED: &str = "the row did not enter its cgroup";
/// The reason every level 2 row records.
pub const IN_MACHINE: &str = "level 2 rows run in a machine";
/// The `cpu.stat` lines kept. Throttling lines need the `cpu` controller, which the box never
/// enables.
const CPU_KEPT: [&str; 3] = ["usage_usec", "user_usec", "system_usec"];
/// The box's own leaf under C.
const BOX_LEAF: &str = "ignorarium-box";

/// A row leaf's counters, raw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CgroupCounters {
    /// `memory.peak`, in bytes.
    pub memory_peak: u64,
    /// Every line of `memory.events`.
    pub memory_events: BTreeMap<String, u64>,
    /// The `CPU_KEPT` lines of `cpu.stat`, in microseconds.
    pub cpu_stat: BTreeMap<String, u64>,
    /// Why the leaf could not be removed, when it could not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept: Option<String>,
}

/// The counters in the texts of `memory.peak`, `memory.events` and `cpu.stat`.
pub fn counters_of(peak: &str, events: &str, cpu_stat: &str) -> Result<CgroupCounters, String> {
    let mut cpu = pairs("cpu.stat", cpu_stat)?;
    cpu.retain(|k, _| CPU_KEPT.contains(&k.as_str()));
    Ok(CgroupCounters {
        memory_peak: count("memory.peak", peak)?,
        memory_events: pairs("memory.events", events)?,
        cpu_stat: cpu,
        kept: None,
    })
}

/// The `key value` lines of a cgroup file.
fn pairs(file: &str, text: &str) -> Result<BTreeMap<String, u64>, String> {
    (text.lines().filter(|l| !l.trim().is_empty()))
        .map(|l| match l.split_once(' ') {
            Some((k, v)) => Ok((k.to_string(), count(file, v)?)),
            None => Err(format!("{file} holds {l:?}, not a key and a count")),
        })
        .collect()
}

fn count(file: &str, text: &str) -> Result<u64, String> {
    (text.trim().parse()).map_err(|_| format!("{file} holds {text:?}, not a count"))
}

/// What setup reads and writes. Fields, so a test can stand in a host.
pub struct CgroupHost {
    /// Where cgroupfs is mounted.
    pub root: PathBuf,
    /// The box's effective user id.
    pub euid: u32,
    /// The text of `/proc/self/cgroup`.
    pub self_cgroup: fn() -> io::Result<String>,
    /// Writes a cgroup file.
    pub write: fn(&Path, &str) -> io::Result<()>,
}

impl CgroupHost {
    /// This host's.
    pub fn host() -> CgroupHost {
        CgroupHost {
            root: "/sys/fs/cgroup".into(),
            euid: rustix::process::geteuid().as_raw(),
            self_cgroup: || std::fs::read_to_string("/proc/self/cgroup"),
            write: |path, text| std::fs::write(path, text),
        }
    }
}

/// Set up C, the box's cgroup, as the parent of the row leaves, and return it. Refuses
/// without writing anything when C is the root, is not the box's to write, or lacks the
/// `memory` controller. The box moves itself out of C first: the kernel enables a controller
/// for C's children only while C holds no process.
pub fn prepare(host: &CgroupHost) -> Result<PathBuf, String> {
    let text = (host.self_cgroup)().map_err(|e| format!("cannot read /proc/self/cgroup: {e}"))?;
    let Some(c) = text.lines().find_map(|l| l.strip_prefix("0::")) else {
        return Err("/proc/self/cgroup names no cgroup v2 path".into());
    };
    if c == "/" {
        return Err("the box runs in the root cgroup; start it in a delegated scope".into());
    }
    let dir = host.root.join(c.trim_start_matches('/'));
    for f in ["cgroup.procs", "cgroup.subtree_control"] {
        let mine = std::fs::metadata(dir.join(f))
            .is_ok_and(|m| m.uid() == host.euid && m.mode() & 0o200 != 0);
        if !mine {
            return Err(format!(
                "{c}/{f} is not the box's to write; start the box in a delegated scope"
            ));
        }
    }
    let controllers = std::fs::read_to_string(dir.join("cgroup.controllers")).unwrap_or_default();
    if !controllers.split_whitespace().any(|n| n == "memory") {
        return Err(format!("the memory controller is not delegated to {c}"));
    }
    let own = dir.join(BOX_LEAF);
    match std::fs::create_dir(&own) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => {
            return Err(format!("cannot make {c}/{BOX_LEAF}: {e}"));
        }
        _ => {}
    }
    let pid = std::process::id().to_string();
    (host.write)(&own.join("cgroup.procs"), &pid)
        .map_err(|e| format!("cannot move the box into {c}/{BOX_LEAF}: {e}"))?;
    (host.write)(&dir.join("cgroup.subtree_control"), "+memory").map_err(|e| {
        if e.kind() == io::ErrorKind::ResourceBusy {
            "the box's cgroup holds other processes; start the box alone in its scope".into()
        } else {
            format!("cannot enable the memory controller in {c}: {e}")
        }
    })?;
    Ok(dir)
}

/// The parent of the row leaves, set up once per process, or why there is none.
pub fn rows_parent() -> &'static Result<PathBuf, String> {
    static PARENT: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    PARENT.get_or_init(|| prepare(&CgroupHost::host()))
}

/// One row's leaf cgroup.
pub struct RowCgroup {
    dir: PathBuf,
    procs: CString,
}

impl RowCgroup {
    /// A new leaf under `rows_parent`. Rows run at once in one process, so each leaf takes a
    /// number of its own.
    pub fn make() -> Result<RowCgroup, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = rows_parent().as_ref().map_err(Clone::clone)?;
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = parent.join(format!("row-{}-{n}", std::process::id()));
        std::fs::create_dir(&dir).map_err(|e| format!("cannot make the row's cgroup: {e}"))?;
        Ok(RowCgroup::at(dir))
    }

    /// The leaf at `dir`, which `make` made or a test stands in.
    pub fn at(dir: PathBuf) -> RowCgroup {
        // A path with a NUL byte gives an empty name, which the hook cannot open.
        let procs = CString::new(dir.join("cgroup.procs").as_os_str().as_bytes());
        RowCgroup {
            procs: procs.unwrap_or_default(),
            dir,
        }
    }

    /// The leaf's folder in cgroupfs.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Make the process `cmd` starts enter this leaf before it execs.
    pub fn enter(&self, cmd: &mut Command) {
        sys::enter_cgroup(cmd, self.procs.clone());
    }

    /// The leaf's counters, once every process in it is reaped; then the leaf is removed. A
    /// leaf whose memory and CPU both read zero never held the row.
    pub fn finish(self) -> Reading<CgroupCounters> {
        let read = |f: &str| {
            std::fs::read_to_string(self.dir.join(f)).map_err(|e| format!("cannot read {f}: {e}"))
        };
        let counters = read("memory.peak")
            .and_then(|peak| counters_of(&peak, &read("memory.events")?, &read("cpu.stat")?));
        let name = self.dir.file_name().unwrap_or_default().to_string_lossy();
        let kept = (std::fs::remove_dir(&self.dir).err()).map(|e| format!("{name} stays: {e}"));
        let usage = |c: &CgroupCounters| c.cpu_stat.get("usage_usec").copied().unwrap_or(0);
        match counters {
            Err(why) => Reading::Unavailable(why),
            Ok(c) if c.memory_peak == 0 && usage(&c) == 0 => Reading::Unavailable(MISSED.into()),
            Ok(c) => Reading::Read(CgroupCounters { kept, ..c }),
        }
    }
}
