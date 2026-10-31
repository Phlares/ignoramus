//! The self-test that runs in a sandbox before every level 1 row. A test that launches a
//! sandbox prints `skip level1: <reason>` and returns where bubblewrap cannot run.
mod common;
use common::{lab, named, plain, skip_level1, write_exec};
use ignorarium::backend::level1::Level1;
use ignorarium::backend::selftest::{self, SelfTest};
use ignorarium::backend::{Backend, Exit, RowJob};
use ignorarium::runner::{self, RunStatus, prepare_with};
use ignorarium::sys;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

const BOX: &str = env!("CARGO_BIN_EXE_ignorarium");

/// A job whose payload leaves `ran` in its output folder.
fn job(d: &Path) -> RowJob {
    for f in ["in", "out", "payload"] {
        std::fs::create_dir(d.join(f)).unwrap();
    }
    write_exec(&d.join("payload/run"), "#!/bin/sh\nprintf yes > /out/ran\n");
    RowJob {
        argv: vec![d.join("payload/run").to_string_lossy().into()],
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        input_dir: d.join("in"),
        output_dir: d.join("out"),
        wall_timeout: Duration::from_secs(30),
        ..RowJob::default()
    }
}

#[test]
fn self_test_fails_on_the_host() {
    // The host has a route out, so the connect check cannot pass.
    let t = selftest::run();
    assert!(!t.connect_blocked);
    assert!(!t.ok());
}

#[test]
fn the_dev_verb_prints_one_json_line_and_exits_102_on_the_host() {
    let out = std::process::Command::new(BOX)
        .args(["dev", "selftest"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(102));
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "{text}");
    let t: SelfTest = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(t, selftest::run());
}

#[test]
fn self_test_passes_inside_the_sandbox() {
    if skip_level1() {
        return;
    }
    let b = Level1::with(Level1::new().bwrap, BOX.into());
    let d = TempDir::new().unwrap();
    let job = job(d.path());
    let o = b.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Ok);
    let all = SelfTest {
        connect_blocked: true,
        vsock_blocked: true,
        io_uring_blocked: true,
        vsock_absent: false,
        io_uring_absent: false,
    };
    // The filter's EPERM, not a missing feature, is what blocks both calls.
    assert_eq!(o.selftest, Some(all));
    assert_eq!(
        std::fs::read_to_string(d.path().join("out/ran")).unwrap(),
        "yes"
    );
    // Nothing the self-test did reaches the payload's output folder.
    assert_eq!(std::fs::read_dir(d.path().join("out")).unwrap().count(), 1);
}

#[test]
fn a_run_logs_the_self_test_of_every_row() {
    if skip_level1() {
        return;
    }
    let lab = lab();
    let dir = plain(&lab, "selftest");
    let b = Level1::with(Level1::new().bwrap, BOX.into());
    let plan = prepare_with(
        &lab.home,
        "selftest",
        &common::work_of(&lab.home, "selftest"),
        vec![Box::new(b)],
    )
    .unwrap();
    assert_eq!(runner::run(&plan).unwrap(), RunStatus::Done);
    let events = common::events_of(&dir);
    let tests = named(&events, "sandbox.selftest");
    assert_eq!(tests.len(), 6);
    for e in tests {
        let a = &e["attrs"];
        assert!(a["row"].is_u64(), "{a}");
        assert_eq!(a["connect_blocked"], true);
        assert_eq!(a["vsock_blocked"], true);
        assert_eq!(a["io_uring_blocked"], true);
        assert_eq!(a["vsock"], "denied");
        assert_eq!(a["io_uring"], "denied");
    }
}

#[test]
fn a_failed_self_test_never_runs_the_payload() {
    if skip_level1() {
        return;
    }
    // The result decides, whatever the exit code: a failed result with exit 0 fails too.
    for code in [102, 0] {
        let d = TempDir::new().unwrap();
        let fake = d.path().join("fake-box");
        write_exec(
            &fake,
            &format!(
                "#!/bin/sh\necho '{{\"connect_blocked\":false,\"vsock_blocked\":true,\"io_uring_blocked\":true}}'\nexit {code}\n"
            ),
        );
        let b = Level1::with(Level1::new().bwrap, fake);
        let work = TempDir::new().unwrap();
        let job = job(work.path());
        let o = b.run_row(&job, &|| true).unwrap();
        assert_eq!(
            o.exit,
            Exit::Infra("sandbox self-test failed: connect was not blocked".into()),
            "exit {code}"
        );
        assert!(!work.path().join("out/ran").exists());
        let t = o.selftest.unwrap();
        assert!(!t.connect_blocked && t.vsock_blocked && t.io_uring_blocked);
    }
}

#[test]
fn a_self_test_that_prints_nothing_never_runs_the_payload() {
    if skip_level1() {
        return;
    }
    let d = TempDir::new().unwrap();
    let fake = d.path().join("fake-box");
    write_exec(&fake, "#!/bin/sh\necho oops >&2\nexit 3\n");
    let b = Level1::with(Level1::new().bwrap, fake);
    let work = TempDir::new().unwrap();
    let job = job(work.path());
    let o = b.run_row(&job, &|| true).unwrap();
    let Exit::Infra(why) = &o.exit else {
        panic!("{:?}", o.exit)
    };
    assert!(why.starts_with("sandbox self-test failed: "), "{why}");
    assert!(why.contains("Payload(3)"), "{why}");
    assert!(!work.path().join("out/ran").exists());
    assert_eq!(o.selftest, None);
}

#[test]
fn io_uring_setup_fails_inside_and_succeeds_or_is_absent_outside() {
    // Outside any filter the answer is the kernel's. Where io_uring is on, it must succeed.
    let kernel = std::fs::read_to_string("/proc/sys/kernel/io_uring_disabled");
    let outside = sys::io_uring_setup_errno();
    if kernel.is_ok_and(|v| v.trim() == "0") {
        assert_eq!(outside, None);
    }
    // Inside the sandbox, the seccomp filter denies it.
    if skip_level1() {
        return;
    }
    let b = Level1::with(Level1::new().bwrap, BOX.into());
    let d = TempDir::new().unwrap();
    let o = b.run_row(&job(d.path()), &|| true).unwrap();
    assert!(o.selftest.unwrap().io_uring_blocked);
}

#[test]
fn the_vsock_check_outside_the_sandbox_is_the_kernels_answer() {
    use rustix::net::{AddressFamily, SocketType, socket};
    let family = AddressFamily::from_raw(libc::AF_VSOCK as u16);
    let errno = socket(family, SocketType::STREAM, None)
        .err()
        .map(|e| e.raw_os_error());
    let kernel = match selftest::Call::of(errno) {
        selftest::Call::Denied => "denied",
        selftest::Call::Absent => "absent",
        selftest::Call::Open => "open",
    };
    assert_eq!(selftest::run().vsock_state(), kernel);
}

#[test]
fn a_row_timeout_during_the_self_test_is_the_rows_timeout() {
    if skip_level1() {
        return;
    }
    let d = TempDir::new().unwrap();
    let slow = d.path().join("slow-box");
    write_exec(
        &slow,
        "#!/bin/sh
sleep 30
",
    );
    let b = Level1::with(Level1::new().bwrap, slow);
    let work = TempDir::new().unwrap();
    let job = RowJob {
        wall_timeout: Duration::from_secs(1),
        ..job(work.path())
    };
    let t = std::time::Instant::now();
    let o = b.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Payload(124));
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    assert!(!work.path().join("out/ran").exists());
}

#[test]
fn a_self_test_that_ends_with_124_while_the_row_has_time_is_a_fault() {
    if skip_level1() {
        return;
    }
    let d = TempDir::new().unwrap();
    let fake = d.path().join("fake-box");
    let passed = r#"{"connect_blocked":true,"vsock_blocked":true,"io_uring_blocked":true}"#;
    write_exec(&fake, &format!("#!/bin/sh\necho '{passed}'\nexit 124\n"));
    let b = Level1::with(Level1::new().bwrap, fake);
    let work = TempDir::new().unwrap();
    let o = b.run_row(&job(work.path()), &|| true).unwrap();
    assert_eq!(
        o.exit,
        Exit::Infra(format!(
            "sandbox self-test failed: it ended with Payload(124): {passed}"
        ))
    );
    assert!(!work.path().join("out/ran").exists());
}

#[test]
fn the_self_tests_cpu_counts_in_the_row() {
    if skip_level1() {
        return;
    }
    // A self-test that burns about 0.4 s of CPU before it reports.
    let d = TempDir::new().unwrap();
    let busy = d.path().join("busy-box");
    write_exec(
        &busy,
        "#!/bin/sh
i=0; while [ $i -lt 400000 ]; do i=$((i+1)); done
         echo '{\"connect_blocked\":true,\"vsock_blocked\":true,\"io_uring_blocked\":true}'
",
    );
    let b = Level1::with(Level1::new().bwrap, busy);
    let work = TempDir::new().unwrap();
    let o = b.run_row(&job(work.path()), &|| true).unwrap();
    assert_eq!(o.exit, Exit::Ok);
    assert!(o.cpu_seconds > 0.1, "{}", o.cpu_seconds);
}
