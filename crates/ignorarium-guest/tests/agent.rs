mod common;

use common::write_exec;
use ignorarium_guest::agent::payload::{self, Ran};
use ignorarium_guest::agent::selftest::{self, route_blocked};
use ignorarium_guest::agent::steps::{
    FLOOR, disable_io_uring, disk_by_serial, disk_bytes, mem_available, mix_seed, now_nanos, room,
    set_clock,
};
use ignorarium_guest::agent::{gate, sandbox};
use ignorarium_guest::pack;
use ignorarium_guest::proto::{End, Job, SEED_ENV, SelfTest};
use rustix::process::{Pid, Signal, kill_process_group};
use std::cell::Cell;
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

const SEED_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

fn job(argv: &[&str], env: &[(&str, &str)], timeout_ms: u64) -> Job {
    Job {
        seed_hex: SEED_HEX.into(),
        unix_nanos: 0,
        argv: argv.iter().map(|s| s.to_string()).collect(),
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        timeout_ms,
        data_limit: None,
    }
}

fn sh(script: &str, out: &Path) -> Job {
    job(
        &["/bin/sh", "-c", script],
        &[("OUT", out.to_str().unwrap())],
        10_000,
    )
}

#[test]
fn mix_seed_writes_the_seed_then_asks_for_a_reseed() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("random");
    let f = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let err = mix_seed(&[7u8; 32], &f).unwrap_err();
    assert_eq!(err.raw_os_error(), Some(libc::ENOTTY));
    assert_eq!(fs::read(&path).unwrap(), vec![7u8; 32]);
}

#[test]
fn set_clock_needs_privilege() {
    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: running as root, set_clock would change the clock");
        return;
    }
    let err = set_clock(now_nanos()).unwrap_err();
    assert_eq!(err.raw_os_error(), Some(libc::EPERM));
}

#[test]
fn disk_by_serial_finds_the_device() {
    let d = tempfile::tempdir().unwrap();
    for (dev, serial) in [("vda", "ign-rootfs\n"), ("vdb", "ign-in\0\0")] {
        fs::create_dir(d.path().join(dev)).unwrap();
        fs::write(d.path().join(dev).join("serial"), serial).unwrap();
    }
    fs::create_dir(d.path().join("loop0")).unwrap();
    let find = |s| disk_by_serial(d.path(), s);
    assert_eq!(find("ign-rootfs").unwrap(), Path::new("/dev/vda"));
    assert_eq!(find("ign-in").unwrap(), Path::new("/dev/vdb"));
    let err = find("ign-out").unwrap_err();
    assert!(err.to_string().contains("ign-out"), "{err}");
}

#[test]
fn disk_bytes_counts_512_byte_sectors() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir(d.path().join("vdc")).unwrap();
    fs::write(d.path().join("vdc/size"), "16384\n").unwrap();
    assert_eq!(
        disk_bytes(d.path(), Path::new("/dev/vdc")).unwrap(),
        16384 * 512
    );
    assert!(disk_bytes(d.path(), Path::new("/dev/vdd")).is_err());
}

#[test]
fn mem_available_reads_meminfo_in_bytes() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("meminfo");
    fs::write(&path, "MemTotal:  2000 kB\nMemAvailable:    1500 kB\n").unwrap();
    assert_eq!(mem_available(&path).unwrap(), 1500 << 10);
    fs::write(&path, "MemTotal:  2000 kB\n").unwrap();
    let e = mem_available(&path).unwrap_err();
    assert_eq!(
        e.to_string(),
        format!("{} names no MemAvailable", path.display())
    );
    assert!(mem_available(Path::new("/proc/meminfo")).unwrap() > 0);
}

#[test]
fn the_guest_refuses_a_row_it_has_no_room_for() {
    let mib = |m: u64| m << 20;
    assert_eq!(FLOOR, mib(128));
    // A 256 MiB cap, /out and /tmp of a 256 MiB disk (128 + 256 MiB) and the floor.
    let need = mib(256 + 384 + 128);
    assert!(room(need, Some(mib(256)), mib(256)).is_ok());
    let e = room(need - 1, Some(mib(256)), mib(256)).unwrap_err();
    assert_eq!(
        e.to_string(),
        "the guest has 767 MiB available at boot, under the 768 MiB the row needs: a data \
         limit of 256 MiB, 384 MiB for /out and /tmp, and a 128 MiB floor"
    );
    // Without a limit: the tmpfs and the floor.
    assert!(room(mib(384 + 128), None, mib(256)).is_ok());
    let e = room(mib(384 + 128) - 1, None, mib(256)).unwrap_err();
    assert!(e.to_string().contains("a data limit of 0 MiB"), "{e}");
}

fn net_with(names: &[&str]) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for n in names {
        fs::create_dir(d.path().join(n)).unwrap();
    }
    d
}

#[test]
fn selftest_only_loopback() {
    let only = |names: &[&str]| {
        selftest::run(net_with(names).path(), || Err(io::Error::other("x"))).only_loopback
    };
    assert!(only(&["lo"]));
    assert!(!only(&["lo", "eth0"]));
    assert!(!only(&["eth0"]));
    assert!(!only(&[]));
}

#[test]
fn selftest_host_path() {
    let d = net_with(&["lo"]);
    let open = selftest::run(d.path(), || Ok(()));
    assert!(!open.host_path_blocked);
    let shut = selftest::run(d.path(), || Err(io::Error::other("refused")));
    assert!(shut.host_path_blocked);
}

#[test]
fn route_is_open_on_a_networked_host() {
    assert!(!route_blocked());
}

#[test]
fn payload_exit_code_and_signal() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        payload::run(&sh("exit 3", d.path())).unwrap().end,
        End::Code(3)
    );
    assert_eq!(
        payload::run(&sh("kill -TERM $$", d.path())).unwrap().end,
        End::Signal(15)
    );
}

/// The soft and hard limit in the `Max data size` line of `/proc/self/limits` text.
fn data_limit(limits: &str) -> Vec<String> {
    let line = limits.lines().find(|l| l.starts_with("Max data size"));
    let words = line.unwrap_or_default().split_whitespace();
    words.skip(3).take(2).map(String::from).collect()
}

#[test]
fn the_data_limit_reaches_the_payload() {
    let d = tempfile::tempdir().unwrap();
    let seen = |limit: Option<u64>| {
        let job = Job {
            data_limit: limit,
            ..sh("cat /proc/self/limits > \"$OUT/limits\"", d.path())
        };
        let ran = payload::run(&job).unwrap();
        assert_eq!((ran.end, ran.data_limit), (End::Code(0), limit));
        data_limit(&fs::read_to_string(d.path().join("limits")).unwrap())
    };
    assert_eq!(seen(Some(256 << 20)), ["268435456", "268435456"]);
    // No limit leaves the agent's own.
    let own = data_limit(&fs::read_to_string("/proc/self/limits").unwrap());
    assert_eq!(own.len(), 2);
    assert_eq!(seen(None), own);
}

#[test]
fn payload_env_is_exactly_the_job_env_plus_the_seed() {
    let d = tempfile::tempdir().unwrap();
    let script = d.path().join("p.sh");
    write_exec(&script, "#!/bin/sh\n/usr/bin/env > \"$OUT/env\"\n");
    let out = d.path().join("out");
    fs::create_dir(&out).unwrap();
    let j = job(
        &[script.to_str().unwrap()],
        &[
            ("A", "1"),
            ("OUT", out.to_str().unwrap()),
            (SEED_ENV, "stale"),
        ],
        10_000,
    );
    assert_eq!(payload::run(&j).unwrap().end, End::Code(0));
    let text = fs::read_to_string(out.join("env")).unwrap();
    let got: BTreeSet<&str> = text
        .lines()
        .filter(|l| !["PWD=", "SHLVL=", "_="].iter().any(|p| l.starts_with(p)))
        .collect();
    let seed_line = format!("{SEED_ENV}={SEED_HEX}");
    let out_line = format!("OUT={}", out.display());
    let want: BTreeSet<&str> = ["A=1", &out_line, &seed_line].into();
    assert_eq!(got, want);
}

#[test]
fn payload_times_out() {
    let d = tempfile::tempdir().unwrap();
    let mut j = sh("exec /bin/sleep 30", d.path());
    j.timeout_ms = 200;
    let ran = payload::run(&j).unwrap();
    assert_eq!(ran.end, End::Timeout);
    assert!(ran.wall < Duration::from_secs(2), "{:?}", ran.wall);
}

#[test]
fn a_leader_that_leaves_its_group_cannot_hang_the_timeout() {
    let d = tempfile::tempdir().unwrap();
    // The leader puts its child in a new group and joins that group, so a kill of its
    // first group reaches neither of them.
    let script = r#"$c = fork // die; if (!$c) { sleep 5; exit }
        setpgrp($c, $c) or die; setpgrp(0, $c) or die;
        open my $f, '>', "$ENV{OUT}/pg" or die; print $f $c; close $f; sleep 5"#;
    let out = d.path().to_str().unwrap();
    let ran = payload::run(&job(&["/usr/bin/perl", "-e", script], &[("OUT", out)], 300)).unwrap();
    let pg: i32 = fs::read_to_string(d.path().join("pg"))
        .unwrap()
        .parse()
        .unwrap();
    let _ = kill_process_group(Pid::from_raw(pg).unwrap(), Signal::KILL);
    assert_eq!(ran.end, End::Timeout);
    assert!(ran.wall < Duration::from_secs(2), "{:?}", ran.wall);
}

#[test]
fn background_children_die_with_the_payload() {
    let d = tempfile::tempdir().unwrap();
    let j = sh(
        "(/bin/sleep 1; /usr/bin/touch \"$OUT/late\") & exit 0",
        d.path(),
    );
    assert_eq!(payload::run(&j).unwrap().end, End::Code(0));
    sleep(Duration::from_millis(1500));
    assert!(!d.path().join("late").exists());
}

#[test]
fn payload_stdio_is_null() {
    let d = tempfile::tempdir().unwrap();
    let script = "exec 3>&1 4>&2\n\
        /usr/bin/readlink /proc/self/fd/0 > \"$OUT/fd0\"\n\
        /usr/bin/readlink /proc/self/fd/3 > \"$OUT/fd1\"\n\
        /usr/bin/readlink /proc/self/fd/4 > \"$OUT/fd2\"";
    assert_eq!(
        payload::run(&sh(script, d.path())).unwrap().end,
        End::Code(0)
    );
    for f in ["fd0", "fd1", "fd2"] {
        assert_eq!(
            fs::read_to_string(d.path().join(f)).unwrap().trim(),
            "/dev/null",
            "{f}"
        );
    }
}

#[test]
fn payload_cpu_seconds_counts_the_child() {
    let d = tempfile::tempdir().unwrap();
    let script = "i=0; while [ $i -lt 1500000 ]; do i=$((i+1)); done";
    let start = Instant::now();
    let ran = payload::run(&sh(script, d.path())).unwrap();
    assert_eq!(ran.end, End::Code(0));
    assert!(
        ran.cpu_seconds > 0.1,
        "{} after {:?}",
        ran.cpu_seconds,
        start.elapsed()
    );
}

#[test]
fn empty_argv_is_an_error() {
    let err = payload::run(&job(&[], &[], 1000)).err().unwrap();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

fn selftest_with(ok: bool) -> SelfTest {
    SelfTest {
        only_loopback: true,
        route_blocked: true,
        host_path_blocked: ok,
        io_uring_errno: Some(libc::EPERM),
    }
}

#[test]
fn selftest_tries_io_uring_setup() {
    let d = net_with(&["lo"]);
    let t = selftest::run(d.path(), || Err(io::Error::other("refused")));
    assert_eq!(
        t.io_uring_errno,
        ignorarium_guest::sys::io_uring_setup_errno()
    );
}

const NO_FDS: &str = "IGNORARIUM_TEST_NO_FDS";

/// Outside a guest the answer is the host kernel's. Where io_uring is on, the call works,
/// and a process with no descriptor left gets `EMFILE`.
#[test]
fn io_uring_setup_errno_is_the_kernels_answer() {
    use ignorarium_guest::sys::io_uring_setup_errno;
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    if std::env::var_os(NO_FDS).is_some() {
        let max = getrlimit(Resource::Nofile).maximum;
        setrlimit(
            Resource::Nofile,
            Rlimit {
                current: Some(0),
                maximum: max,
            },
        )
        .unwrap();
        assert_eq!(io_uring_setup_errno(), Some(libc::EMFILE));
        return;
    }
    let switch = fs::read_to_string("/proc/sys/kernel/io_uring_disabled");
    if !switch.is_ok_and(|v| v.trim() == "0") {
        println!("skip: io_uring is off or absent on this kernel");
        return;
    }
    assert_eq!(io_uring_setup_errno(), None);
    let me = std::env::current_exe().unwrap();
    let child = std::process::Command::new(me)
        .args(["--exact", "io_uring_setup_errno_is_the_kernels_answer"])
        .env(NO_FDS, "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&child.stdout);
    assert!(
        child.status.success() && text.contains("1 passed"),
        "{text}"
    );
}

#[test]
fn the_guest_counts_io_uring_as_blocked_only_when_refused() {
    // The filter's answer, or a kernel without the call; any other errno leaves it open.
    let cases = [
        (Some(libc::EPERM), true),
        (Some(libc::ENOSYS), true),
        (Some(libc::EAFNOSUPPORT), true),
        (Some(libc::EACCES), false),
        (Some(libc::EINVAL), false),
        (None, false),
    ];
    for (io_uring_errno, ok) in cases {
        let t = SelfTest {
            io_uring_errno,
            ..selftest_with(true)
        };
        assert_eq!(t.ok(), ok, "{io_uring_errno:?}");
    }
}

#[test]
fn disable_io_uring_sets_the_switch_for_every_process() {
    let d = tempfile::tempdir().unwrap();
    let switch = d.path().join("io_uring_disabled");
    fs::write(&switch, "0").unwrap();
    disable_io_uring(&switch).unwrap();
    assert_eq!(fs::read_to_string(&switch).unwrap(), "2");
    // A kernel without the switch is left as it is.
    disable_io_uring(&d.path().join("absent")).unwrap();
    assert!(!d.path().join("absent").exists());
    assert!(disable_io_uring(d.path()).is_err());
}

#[test]
fn payload_runs_in_the_folder_it_is_given() {
    let d = tempfile::tempdir().unwrap();
    let j = sh("pwd > \"$OUT/pwd\"; echo rel > relative", d.path());
    assert_eq!(
        payload::run_as(&j, None, d.path()).unwrap().end,
        End::Code(0)
    );
    let pwd = fs::read_to_string(d.path().join("pwd")).unwrap();
    assert_eq!(Path::new(pwd.trim()), d.path());
    assert_eq!(
        fs::read_to_string(d.path().join("relative")).unwrap(),
        "rel
"
    );
}

fn ran_with(packed: Result<&str, &str>) -> io::Result<(Ran, Result<String, String>)> {
    let ran = Ran {
        end: End::Code(0),
        wall: Duration::from_millis(5),
        cpu_seconds: 0.25,
        data_limit: Some(7 << 20),
    };
    Ok((ran, packed.map(str::to_string).map_err(str::to_string)))
}

#[test]
fn gate_runs_the_payload_only_after_a_passing_selftest() {
    let called = Cell::new(false);
    let ran = || {
        called.set(true);
        ran_with(Ok("abc"))
    };
    let failed = gate(selftest_with(false), 42, ran).unwrap();
    assert!(!called.get());
    assert_eq!((failed.end, failed.output_hash.as_str()), (None, ""));
    assert_eq!(failed.clock_unix_nanos, 42);
    assert_eq!(failed.pack_refused, None);
    // No payload ran, so no limit was applied.
    assert_eq!(failed.data_limit, None);

    let passed = gate(selftest_with(true), 42, ran).unwrap();
    assert!(called.get());
    assert_eq!(passed.end, Some(End::Code(0)));
    assert_eq!(passed.output_hash, "abc");
    assert_eq!(passed.payload_wall_ns, 5_000_000);
    assert_eq!(passed.cpu_seconds, 0.25);
    assert_eq!(passed.pack_refused, None);
    assert_eq!(passed.data_limit, Some(7 << 20));
}

#[test]
fn gate_reports_a_refused_pack_as_the_payloads_result() {
    let r = gate(selftest_with(true), 42, || ran_with(Err("too big"))).unwrap();
    assert_eq!(r.end, Some(End::Code(0)));
    assert_eq!(r.output_hash, "");
    assert_eq!(r.pack_refused.as_deref(), Some("too big"));
    assert_eq!(r.cpu_seconds, 0.25);
}

#[test]
fn the_view_refuses_an_output_size_that_leaves_out_uncapped() {
    // A tmpfs cap of 0 is no cap, and below two of the largest entries the inode cap is 0.
    let d = tempfile::tempdir().unwrap();
    let view = d.path().join("view");
    let none = Path::new("/nonexistent");
    for out_bytes in [0, 2 * pack::ENTRY_MAX - 1] {
        let err = sandbox::build(none, &view, none, none, out_bytes).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{err}");
        assert!(!view.exists());
    }
    if rustix::process::geteuid().is_root() {
        eprintln!("skipped the upper side: running as root, the build would mount");
        return;
    }
    // Past the check, the first mount needs root.
    let err = sandbox::build(none, &view, none, none, 2 * pack::ENTRY_MAX).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
}
