mod common;

use common::{skip_cgroup, skip_level1, write_exec};
use ignorarium::backend::cgroup::{
    CgroupCounters, CgroupHost, MISSED, RowCgroup, counters_of, prepare, rows_parent,
};
use ignorarium::backend::level0::Level0;
use ignorarium::backend::level1::Level1;
use ignorarium::backend::{Backend, Exit, RowJob, RowOutcome};
use ignorarium::canon::sha256_hex;
use ignorarium::telemetry::{
    CpuTicks, HostCpu, Reading, cpu_ticks, cpu_ticks_of, host_cpu_of, record,
};
use serde_json::json;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

const STAT: &str = "cpu  10132153 290696 3084719 46828483 16683 1 25195 175628 7 3
cpu0 5066076 145348 1542359 23414241 8341 0 12597 87814 3 1
intr 1234 5 6
ctxt 99
";

#[test]
fn cpu_ticks_of_reads_the_aggregate_line() {
    let want = vec![
        10_132_153, 290_696, 3_084_719, 46_828_483, 16_683, 1, 25_195, 175_628, 7, 3,
    ];
    assert_eq!(
        cpu_ticks_of(STAT, 100),
        Reading::Read(CpuTicks {
            user_hz: 100,
            ticks: want
        })
    );
    // Only per-CPU lines: no aggregate line.
    let per_cpu = "cpu0 1 2 3 4 5 6 7 8 9 10\ncpu1 1 2 3 4 5 6 7 8 9 10\n";
    assert!(matches!(
        cpu_ticks_of(per_cpu, 100),
        Reading::Unavailable(_)
    ));
    assert!(matches!(cpu_ticks_of("", 100), Reading::Unavailable(_)));
    // An old kernel gives 7 fields.
    assert_eq!(
        cpu_ticks_of("cpu  1 2 3 4 5 6 7\ncpu0 1 2 3 4 5 6 7\n", 250),
        Reading::Read(CpuTicks {
            user_hz: 250,
            ticks: vec![1, 2, 3, 4, 5, 6, 7]
        })
    );
    // A field that is no number is unavailable, not a zero.
    assert!(matches!(
        cpu_ticks_of("cpu  1 2 x 4\n", 100),
        Reading::Unavailable(_)
    ));
    // A cpu line with no counts is unavailable, not an empty read.
    assert!(matches!(
        cpu_ticks_of("cpu\n", 100),
        Reading::Unavailable(_)
    ));
}

#[test]
fn cpu_ticks_reads_this_host_at_its_tick_rate() {
    // As in the hook test below: no child process before the rows' parent is set up.
    let _ = rows_parent();
    let getconf = std::process::Command::new("getconf")
        .arg("CLK_TCK")
        .output()
        .unwrap();
    let hz: u64 = String::from_utf8(getconf.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let Reading::Read(now) = cpu_ticks() else {
        panic!("this host gives no CPU counts");
    };
    assert_eq!(now.user_hz, hz);
    // user nice system idle iowait irq softirq, since Linux 2.6.
    assert!(now.ticks.len() >= 7, "{now:?}");
}

const X86: &str = "processor\t: 0
vendor_id\t: GenuineIntel
model name\t: Intel Core Processor (Haswell, no TSX)
flags\t\t: fpu vme de pse tsc
bugs\t\t: cpu_meltdown

processor\t: 1
model name\t: Another Model
flags\t\t: other flags
";

const AARCH64: &str = "processor\t: 0
BogoMIPS\t: 50.00
Features\t: fp asimd evtstrm
CPU implementer\t: 0x41
";

#[test]
fn host_cpu_of_reads_model_and_hashes_flags() {
    assert_eq!(
        host_cpu_of(X86),
        Reading::Read(HostCpu {
            model: "Intel Core Processor (Haswell, no TSX)".into(),
            flags_sha256: sha256_hex(b"fpu vme de pse tsc"),
        })
    );
    assert_eq!(
        host_cpu_of(AARCH64),
        Reading::Unavailable("/proc/cpuinfo names no model name".into())
    );
    // A model with no flags cannot be fingerprinted.
    assert!(matches!(
        host_cpu_of("model name\t: M\n"),
        Reading::Unavailable(_)
    ));
}

#[test]
fn a_reading_serializes_as_its_value_or_as_its_reason() {
    let read = Reading::Read(vec![1, 2]);
    assert_eq!(serde_json::to_value(&read).unwrap(), json!([1, 2]));
    let gone: Reading<u8> = Reading::Unavailable("why".into());
    assert_eq!(
        serde_json::to_value(&gone).unwrap(),
        json!({ "unavailable": "why" })
    );
}

#[test]
fn the_record_holds_both_tick_reads_and_redacts_reasons() {
    let at = |n: u64| {
        Reading::Read(CpuTicks {
            user_hz: 100,
            ticks: vec![n, n + 1],
        })
    };
    let host = Reading::Read(HostCpu {
        model: "m".into(),
        flags_sha256: "f".into(),
    });
    let keep = |s: &str| s.to_string();
    let counters = counters_of(
        "7
",
        "oom_kill 0
",
        "usage_usec 3
",
    )
    .unwrap();
    let v = record(&host, &at(5), &at(9), &Reading::Read(counters), &keep);
    assert_eq!(
        v,
        json!({
            "host_cpu": { "model": "m", "flags_sha256": "f" },
            "cpu_ticks": { "user_hz": 100, "start": [5, 6], "end": [9, 10] },
            "cgroup": {
                "memory_peak": 7,
                "memory_events": { "oom_kill": 0 },
                "cpu_stat": { "usage_usec": 3 },
            },
        })
    );
    // One failed read makes the whole pair unavailable; every reason is redacted.
    let gone: Reading<CpuTicks> = Reading::Unavailable("read /srv/x failed".into());
    let hide = |s: &str| s.replace("/srv/x", "<home>");
    let none: Reading<HostCpu> = Reading::Unavailable("no /srv/x".into());
    let no_cgroup: Reading<CgroupCounters> = Reading::Unavailable("not in /srv/x".into());
    let v = record(&none, &at(5), &gone, &no_cgroup, &hide);
    assert_eq!(
        v["cpu_ticks"],
        json!({ "unavailable": "read <home> failed" })
    );
    assert_eq!(v["host_cpu"], json!({ "unavailable": "no <home>" }));
    assert_eq!(v["cgroup"], json!({ "unavailable": "not in <home>" }));
    let v = record(&host, &gone, &at(5), &no_cgroup, &hide);
    assert_eq!(
        v["cpu_ticks"],
        json!({ "unavailable": "read <home> failed" })
    );
}

// ---- the row cgroup ----

const MEMORY_EVENTS: &str = "low 0
high 0
max 0
oom 0
oom_kill 0
oom_group_kill 0
";

const CPU_STAT: &str = "usage_usec 51234
user_usec 40000
system_usec 11234
nice_usec 0
core_sched.force_idle_usec 0
";

const THROTTLE: &str = "nr_periods 10
nr_throttled 2
throttled_usec 900
nr_bursts 0
burst_usec 0
";

#[test]
fn counters_of_parses_kernel_text() {
    let c = counters_of("73400320\n", MEMORY_EVENTS, CPU_STAT).unwrap();
    assert_eq!(c.memory_peak, 73_400_320);
    let events: Vec<_> = (c.memory_events.iter())
        .map(|(k, v)| (k.as_str(), *v))
        .collect();
    assert_eq!(
        events,
        [
            ("high", 0),
            ("low", 0),
            ("max", 0),
            ("oom", 0),
            ("oom_group_kill", 0),
            ("oom_kill", 0)
        ]
    );
    // Only the core CPU times: no throttling, no other line.
    let cpu = BTreeMap::from([
        ("system_usec".to_string(), 11_234),
        ("usage_usec".to_string(), 51_234),
        ("user_usec".to_string(), 40_000),
    ]);
    assert_eq!(c.cpu_stat, cpu);
    assert_eq!(c.kept, None);
    let throttled = counters_of(
        "73400320\n",
        MEMORY_EVENTS,
        &format!("{CPU_STAT}{THROTTLE}"),
    );
    assert_eq!(throttled.unwrap(), c);
    // A text that is not a count is an error, never a zero.
    assert!(counters_of("max\n", MEMORY_EVENTS, CPU_STAT).is_err());
    assert!(counters_of("1\n", "oom_kill x\n", CPU_STAT).is_err());
    assert!(counters_of("1\n", "oom_kill\n", CPU_STAT).is_err());
    assert!(counters_of("1\n", MEMORY_EVENTS, "usage_usec -1\n").is_err());
}

thread_local! {
    /// The writes the `noted` seam saw on this thread.
    static WRITES: RefCell<Vec<(PathBuf, String)>> = const { RefCell::new(Vec::new()) };
}

fn noted(path: &Path, text: &str) -> io::Result<()> {
    WRITES.with(|w| w.borrow_mut().push((path.into(), text.into())));
    Ok(())
}

/// `noted`, but enabling a controller fails as it does while the cgroup holds a process.
fn busy(path: &Path, text: &str) -> io::Result<()> {
    noted(path, text)?;
    if path.ends_with("cgroup.subtree_control") {
        return Err(io::Error::from_raw_os_error(libc::EBUSY));
    }
    Ok(())
}

fn writes() -> Vec<(PathBuf, String)> {
    WRITES.with(RefCell::take)
}

fn in_scope() -> io::Result<String> {
    Ok("0::/app.slice/run-1.scope\n".into())
}

/// A cgroupfs in a temp folder. The box's cgroup is `/app.slice/run-1.scope`, its files are
/// the test's own, and it offers `controllers`.
fn cgroupfs(controllers: &str) -> (TempDir, PathBuf) {
    let root = TempDir::new().unwrap();
    let c = root.path().join("app.slice/run-1.scope");
    std::fs::create_dir_all(&c).unwrap();
    for f in ["cgroup.procs", "cgroup.subtree_control"] {
        std::fs::write(c.join(f), "").unwrap();
    }
    std::fs::write(c.join("cgroup.controllers"), controllers).unwrap();
    (root, c)
}

fn host(
    root: &TempDir,
    euid: u32,
    self_cgroup: fn() -> io::Result<String>,
    write: fn(&Path, &str) -> io::Result<()>,
) -> CgroupHost {
    CgroupHost {
        root: root.path().into(),
        euid,
        self_cgroup,
        write,
    }
}

#[test]
fn prepare_refuses_the_root_a_foreign_owner_and_a_missing_memory_controller() {
    let (root, c) = cgroupfs("cpu memory pids\n");
    let me = std::fs::metadata(c.join("cgroup.procs")).unwrap().uid();
    let refused = |h: CgroupHost| prepare(&h).unwrap_err();
    let at_root = refused(host(&root, me, || Ok("0::/\n".into()), noted));
    assert!(at_root.contains("the root cgroup"), "{at_root}");
    let v1 = refused(host(&root, me, || Ok("1:name=systemd:/x\n".into()), noted));
    assert!(v1.contains("no cgroup v2"), "{v1}");
    let foreign = refused(host(&root, me + 1, in_scope, noted));
    assert!(foreign.contains("not the box's to write"), "{foreign}");
    let control = c.join("cgroup.subtree_control");
    std::fs::set_permissions(&control, std::fs::Permissions::from_mode(0o444)).unwrap();
    let read_only = refused(host(&root, me, in_scope, noted));
    assert!(read_only.contains("not the box's to write"), "{read_only}");
    std::fs::set_permissions(&control, std::fs::Permissions::from_mode(0o644)).unwrap();
    let (no_memory, _) = cgroupfs("cpu pids\n");
    let missing = refused(host(&no_memory, me, in_scope, noted));
    assert!(missing.contains("memory controller"), "{missing}");
    // A refusal writes nothing and makes no folder.
    assert_eq!(writes(), []);
    assert!(!c.join("ignorarium-box").exists());
}

#[test]
fn prepare_moves_the_box_before_it_enables_memory() {
    let (root, c) = cgroupfs("cpu memory pids\n");
    let me = std::fs::metadata(c.join("cgroup.procs")).unwrap().uid();
    writes();
    assert_eq!(prepare(&host(&root, me, in_scope, noted)), Ok(c.clone()));
    let moved = (
        c.join("ignorarium-box/cgroup.procs"),
        std::process::id().to_string(),
    );
    let enabled = (c.join("cgroup.subtree_control"), "+memory".to_string());
    assert_eq!(writes(), [moved.clone(), enabled.clone()]);
    assert!(c.join("ignorarium-box").is_dir());
    // A resumed run in the same scope finds the box's folder there.
    assert_eq!(prepare(&host(&root, me, in_scope, noted)), Ok(c.clone()));
    assert_eq!(writes(), [moved.clone(), enabled.clone()]);
    assert_eq!(
        prepare(&host(&root, me, in_scope, busy)),
        Err("the box's cgroup holds other processes; start the box alone in its scope".into())
    );
    assert_eq!(writes(), [moved, enabled]);
}

#[test]
fn prepare_says_when_it_cannot_make_the_box_leaf() {
    let (root, c) = cgroupfs("cpu memory pids\n");
    let me = std::fs::metadata(c.join("cgroup.procs")).unwrap().uid();
    if me == 0 {
        println!("skip: root can make a folder in a read-only one");
        return;
    }
    writes();
    std::fs::set_permissions(&c, std::fs::Permissions::from_mode(0o555)).unwrap();
    let refused = prepare(&host(&root, me, in_scope, noted));
    std::fs::set_permissions(&c, std::fs::Permissions::from_mode(0o755)).unwrap();
    let why = refused.unwrap_err();
    assert!(
        why.contains("cannot make /app.slice/run-1.scope/ignorarium-box: "),
        "{why}"
    );
    assert_eq!(writes(), []);
}

/// A leaf in a temp folder with these counters. Its files stay in it, so it cannot be removed.
fn leaf_with(peak: u64, usage: u64) -> (TempDir, RowCgroup) {
    let d = TempDir::new().unwrap();
    let leaf = d.path().join("leaf");
    std::fs::create_dir(&leaf).unwrap();
    std::fs::write(leaf.join("memory.peak"), format!("{peak}\n")).unwrap();
    std::fs::write(leaf.join("memory.events"), MEMORY_EVENTS).unwrap();
    std::fs::write(leaf.join("cpu.stat"), format!("usage_usec {usage}\n")).unwrap();
    (d, RowCgroup::at(leaf))
}

#[test]
fn a_leaf_missed_the_row_only_when_memory_and_cpu_both_read_zero() {
    let finish = |peak, usage| {
        let (_d, leaf) = leaf_with(peak, usage);
        leaf.finish()
    };
    assert_eq!(finish(0, 0), Reading::Unavailable(MISSED.into()));
    for (peak, usage) in [(4096, 0), (0, 12), (4096, 12)] {
        let Reading::Read(c) = finish(peak, usage) else {
            panic!("peak {peak}, usage {usage}: unavailable");
        };
        assert_eq!((c.memory_peak, c.cpu_stat["usage_usec"]), (peak, usage));
    }
}

#[test]
fn a_leaf_that_cannot_be_removed_says_why() {
    let (_d, leaf) = leaf_with(4096, 12);
    let Reading::Read(c) = leaf.finish() else {
        panic!("unavailable");
    };
    let kept = c.kept.unwrap();
    assert!(kept.starts_with("leaf stays: "), "{kept}");
}

const LONG: Duration = Duration::from_secs(30);
const BOX: &str = env!("CARGO_BIN_EXE_ignorarium");
/// Fills a 64 MiB buffer.
const DD: &str = "dd if=/dev/zero of=/dev/null bs=64M count=1 2>/dev/null";
const FIXED: &str = "printf 42 > \"$IGNORARIUM_OUT/result\"";

/// A job whose payload, a script alone in its folder, runs `script` with `/bin/sh`.
fn row(script: &str) -> (TempDir, RowJob) {
    let d = TempDir::new().unwrap();
    for f in ["in", "out", "payload"] {
        std::fs::create_dir(d.path().join(f)).unwrap();
    }
    let payload = d.path().join("payload/run");
    write_exec(&payload, &format!("#!/bin/sh\n{script}\n"));
    let job = RowJob {
        argv: vec![payload.to_string_lossy().into()],
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        input_dir: d.path().join("in"),
        output_dir: d.path().join("out"),
        wall_timeout: LONG,
        ..RowJob::default()
    };
    (d, job)
}

/// A level that runs a row in the cgroup it is given.
enum At {
    Zero,
    One(Level1),
}

impl At {
    fn level(&self) -> u8 {
        match self {
            At::Zero => Level0.level(),
            At::One(b) => b.level(),
        }
    }

    fn run(&self, job: &RowJob, leaf: impl FnOnce() -> Result<RowCgroup, String>) -> RowOutcome {
        match self {
            At::Zero => Level0.run_in(job, &|| true, leaf),
            At::One(b) => b.run_in(job, &|| true, leaf),
        }
    }
}

/// Level 0, and level 1 unless the reason this host cannot run it is printed.
fn levels() -> Vec<At> {
    let mut levels = vec![At::Zero];
    if !skip_level1() {
        levels.push(At::One(Level1::with(Level1::new().bwrap, BOX.into())));
    }
    levels
}

fn off() -> Result<RowCgroup, String> {
    Err("off".into())
}

#[test]
fn a_cgroup_hook_that_cannot_open_its_file_does_not_fail_the_row() {
    // The rows' parent is set up before this test starts a process: a child left in the
    // box's cgroup while another test sets it up makes that setup refuse.
    let _ = rows_parent();
    let d = TempDir::new().unwrap();
    // A leaf whose counters read zero and which has no `cgroup.procs` to open.
    let leaf = d.path().join("leaf");
    std::fs::create_dir(&leaf).unwrap();
    std::fs::write(leaf.join("memory.peak"), "0\n").unwrap();
    std::fs::write(leaf.join("memory.events"), MEMORY_EVENTS).unwrap();
    let zero = "usage_usec 0\nuser_usec 0\nsystem_usec 0\n";
    std::fs::write(leaf.join("cpu.stat"), zero).unwrap();
    let (_a, without) = row(FIXED);
    let (_b, with) = row(FIXED);
    let no_leaf = Level0.run_in(&without, &|| true, off);
    let missed = Level0.run_in(&with, &|| true, || Ok(RowCgroup::at(leaf)));
    assert_eq!(no_leaf.exit, Exit::Ok);
    assert_eq!(no_leaf.cgroup, Reading::Unavailable("off".into()));
    assert_eq!(missed.exit, Exit::Ok);
    assert_eq!(missed.output_hash, no_leaf.output_hash);
    assert_eq!(missed.cgroup, Reading::Unavailable(MISSED.into()));
}

#[test]
fn level0_and_level1_rows_record_their_peak_memory() {
    if skip_cgroup() {
        return;
    }
    for at in levels() {
        let (_d, job) = row(DD);
        let o = at.run(&job, RowCgroup::make);
        let level = at.level();
        assert_eq!(o.exit, Exit::Ok, "level {level}");
        let Reading::Read(c) = &o.cgroup else {
            panic!("level {level}: {:?}", o.cgroup);
        };
        assert!(
            (64 << 20..1 << 30).contains(&c.memory_peak),
            "level {level}: {c:?}"
        );
        assert_eq!(c.memory_events["oom_kill"], 0, "level {level}");
        assert!(c.cpu_stat["usage_usec"] > 0, "level {level}");
    }
}

#[test]
fn a_row_leaf_is_removed_after_the_row() {
    if skip_cgroup() {
        return;
    }
    for at in levels() {
        let (_d, job) = row(FIXED);
        let leaf = RowCgroup::make().unwrap();
        let dir = leaf.dir().to_path_buf();
        assert!(dir.is_dir());
        let o = at.run(&job, || Ok(leaf));
        let level = at.level();
        assert!(
            matches!(&o.cgroup, Reading::Read(c) if c.kept.is_none()),
            "level {level}: {:?}",
            o.cgroup
        );
        assert!(!dir.exists(), "level {level}");
    }
}

#[test]
fn a_row_gives_the_same_exit_and_hash_with_and_without_its_cgroup() {
    if skip_cgroup() {
        return;
    }
    for at in levels() {
        let (_a, without) = row(FIXED);
        let (_b, with) = row(FIXED);
        let no_leaf = at.run(&without, off);
        let leaf = at.run(&with, RowCgroup::make);
        let level = at.level();
        assert!(
            matches!(leaf.cgroup, Reading::Read(_)),
            "level {level}: {:?}",
            leaf.cgroup
        );
        assert_eq!(no_leaf.exit, Exit::Ok, "level {level}");
        assert_eq!(leaf.exit, no_leaf.exit, "level {level}");
        assert_eq!(leaf.output_hash, no_leaf.output_hash, "level {level}");
    }
}
