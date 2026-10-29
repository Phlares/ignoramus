mod common;

use common::write_exec;
use ignorarium_guest::agent::gate;
use ignorarium_guest::agent::payload::{self, Ran};
use ignorarium_guest::agent::selftest::{self, route_blocked};
use ignorarium_guest::agent::steps::{disk_by_serial, mix_seed, now_nanos, set_clock};
use ignorarium_guest::proto::{End, Job, SEED_ENV, SelfTest};
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
    assert!(!only(&[]));
}

#[test]
fn selftest_host_path() {
    let d = net_with(&["lo"]);
    let open = selftest::run(d.path(), || Ok(()));
    assert!(!open.host_path_blocked);
    assert!(!open.ok());
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
    }
}

#[test]
fn gate_runs_the_payload_only_after_a_passing_selftest() {
    let called = Cell::new(false);
    let ran = || {
        called.set(true);
        Ok((
            Ran {
                end: End::Code(0),
                wall: Duration::from_millis(5),
                cpu_seconds: 0.25,
            },
            "abc".to_string(),
        ))
    };
    let failed = gate(selftest_with(false), 42, ran).unwrap();
    assert!(!called.get());
    assert_eq!((failed.end, failed.output_hash.as_str()), (None, ""));
    assert_eq!(failed.clock_unix_nanos, 42);

    let passed = gate(selftest_with(true), 42, ran).unwrap();
    assert!(called.get());
    assert_eq!(passed.end, Some(End::Code(0)));
    assert_eq!(passed.output_hash, "abc");
    assert_eq!(passed.payload_wall_ns, 5_000_000);
    assert_eq!(passed.cpu_seconds, 0.25);
}
