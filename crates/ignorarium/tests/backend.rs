use ignorarium::backend::level0::{Level0, output_hash};
use ignorarium::backend::{Backend, Exit, Probe, RowJob, Watch};
use ignorarium::sys;
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::time::Duration;

fn job(dir: &std::path::Path, script: &str, timeout: Duration) -> RowJob {
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    RowJob {
        argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
        env: vec![("OUT".into(), out.to_string_lossy().into())],
        input_dir: dir.join("in"),
        output_dir: out,
        seed: [0; 32],
        wall_timeout: timeout,
        rootfs: None,
    }
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

#[test]
fn level0_runs_hashes_output_and_times_the_row() {
    let d = tempfile::tempdir().unwrap();
    let o = Level0
        .run_row(
            &job(
                d.path(),
                "printf 42 > \"$OUT/result\"",
                Duration::from_secs(30),
            ),
            &|| true,
        )
        .unwrap();
    assert_eq!(o.exit, Exit::Ok);
    assert!(o.selftest_ok);
    assert!(o.wall_seconds > 0.0 && o.wall_seconds < 30.0);
    assert!(o.cpu_seconds >= 0.0 && o.cpu_seconds < 30.0);
    let manifest = format!("[[\"result\",\"file\",\"{}\"]]", sha(b"42"));
    assert_eq!(o.output_hash, sha(manifest.as_bytes()));
}

#[test]
fn level0_marks_a_nonzero_exit_a_payload_failure() {
    let d = tempfile::tempdir().unwrap();
    let o = Level0
        .run_row(&job(d.path(), "exit 3", Duration::from_secs(30)), &|| true)
        .unwrap();
    assert_eq!(o.exit, Exit::Payload(3));
}

#[test]
fn level0_clears_the_environment() {
    let d = tempfile::tempdir().unwrap();
    let o = Level0
        .run_row(
            &job(
                d.path(),
                "tr '\\0' '\\n' </proc/$$/environ > \"$OUT/env\"",
                Duration::from_secs(30),
            ),
            &|| true,
        )
        .unwrap();
    assert_eq!(o.exit, Exit::Ok);
    let seen = std::fs::read_to_string(d.path().join("out/env")).unwrap();
    // The backend adds the two path variables; nothing else leaks in.
    let mut lines: Vec<&str> = seen.lines().collect();
    lines.sort();
    let dir = d.path().display();
    assert_eq!(
        lines,
        vec![
            format!("IGNORARIUM_IN={dir}/in"),
            format!("IGNORARIUM_OUT={dir}/out"),
            format!("OUT={dir}/out"),
        ]
    );
}

#[test]
fn level0_kills_the_group_at_the_wall_timeout() {
    let d = tempfile::tempdir().unwrap();
    let o = Level0
        .run_row(
            &job(
                d.path(),
                "(sleep 3; printf late > \"$OUT/late\") & sleep 30",
                Duration::from_millis(300),
            ),
            &|| true,
        )
        .unwrap();
    assert_eq!(o.exit, Exit::Payload(124));
    assert!(
        o.wall_seconds >= 0.3 && o.wall_seconds < 2.5,
        "{}",
        o.wall_seconds
    );
    std::thread::sleep(Duration::from_millis(3500));
    assert!(!d.path().join("out/late").exists());
}

#[test]
fn level0_reports_a_missing_program_as_infra() {
    let d = tempfile::tempdir().unwrap();
    let mut j = job(d.path(), "", Duration::from_secs(5));
    j.argv = vec!["/nonexistent/prog".into()];
    assert!(matches!(
        Level0.run_row(&j, &|| true).unwrap().exit,
        Exit::Infra(_)
    ));
    j.argv = vec![];
    assert_eq!(
        Level0.run_row(&j, &|| true).unwrap().exit,
        Exit::Infra("empty argv".into())
    );
}

#[test]
fn level0_reports_a_signal_death_as_128_plus_signal() {
    let d = tempfile::tempdir().unwrap();
    let o = Level0
        .run_row(
            &job(d.path(), "kill -9 $$", Duration::from_secs(30)),
            &|| true,
        )
        .unwrap();
    assert_eq!(o.exit, Exit::Payload(137));
}

#[test]
fn level0_counts_cpu_time() {
    let d = tempfile::tempdir().unwrap();
    // About 0.4 s of cpu. Under one second, the microseconds field holds the whole count.
    let o = Level0
        .run_row(
            &job(
                d.path(),
                "i=0; while [ $i -lt 400000 ]; do i=$((i+1)); done",
                Duration::from_secs(60),
            ),
            &|| true,
        )
        .unwrap();
    assert_eq!(o.exit, Exit::Ok);
    assert!(o.cpu_seconds > 0.05, "{}", o.cpu_seconds);
    assert!(o.cpu_seconds <= o.wall_seconds + 0.05);
}

#[test]
fn level0_probe_and_level() {
    assert_eq!(Level0.probe(), Probe::Available);
    assert_eq!(Level0.level(), 0);
}

#[test]
fn output_hash_records_links_and_nested_files_without_following() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("a/b")).unwrap();
    std::fs::write(d.path().join("a/b/c"), b"x").unwrap();
    std::fs::write(d.path().join("z"), b"y").unwrap();
    std::os::unix::fs::symlink("/etc/hostname", d.path().join("a/ln")).unwrap();
    let m = format!(
        "[[\"a/b/c\",\"file\",\"{}\"],[\"a/ln\",\"link\",\"\"],[\"z\",\"file\",\"{}\"]]",
        sha(b"x"),
        sha(b"y")
    );
    assert_eq!(output_hash(d.path()).unwrap(), sha(m.as_bytes()));
}

#[test]
fn registered_starts_with_level0() {
    assert_eq!(ignorarium::backend::registered()[0].level(), 0);
}

#[test]
fn level0_kills_a_background_grandchild_when_the_leader_exits() {
    let d = tempfile::tempdir().unwrap();
    let pidfile = d.path().join("out/pid");
    let script = format!(
        "(sleep 0.3; printf late > \"$OUT/late\") & echo $! > {}",
        pidfile.display()
    );
    let o = Level0
        .run_row(&job(d.path(), &script, Duration::from_secs(30)), &|| true)
        .unwrap();
    assert_eq!(o.exit, Exit::Ok);
    std::thread::sleep(Duration::from_millis(800));
    assert!(!d.path().join("out/late").exists());
    let pid = std::fs::read_to_string(&pidfile).unwrap();
    // A killed orphan may linger as a zombie until its new parent reaps it.
    let status =
        std::fs::read_to_string(format!("/proc/{}/status", pid.trim())).unwrap_or_default();
    assert!(
        status.is_empty() || status.contains("State:\tZ"),
        "{status}"
    );
    // The hash holds only the pid file: no late write.
    let m = format!("[[\"pid\",\"file\",\"{}\"]]", sha(pid.as_bytes()));
    assert_eq!(o.output_hash, sha(m.as_bytes()));
}

#[test]
fn a_fifo_in_the_output_is_kind_other_and_does_not_block() {
    let d = tempfile::tempdir().unwrap();
    let st = std::process::Command::new("mkfifo")
        .arg(d.path().join("p"))
        .status()
        .unwrap();
    assert!(st.success());
    let m = "[[\"p\",\"other\",\"\"]]";
    assert_eq!(output_hash(d.path()).unwrap(), sha(m.as_bytes()));
}

#[test]
fn a_non_utf8_name_is_recorded_as_hex_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path()
            .join(std::ffi::OsStr::from_bytes(&[b'a', 0xff, b'b'])),
        b"x",
    )
    .unwrap();
    std::fs::write(d.path().join("a?b"), b"x").unwrap();
    let m = format!(
        "[[\"!bytes:61ff62\",\"file\",\"{0}\"],[\"a?b\",\"file\",\"{0}\"]]",
        sha(b"x")
    );
    assert_eq!(output_hash(d.path()).unwrap(), sha(m.as_bytes()));
}

#[test]
fn a_symlinked_output_dir_is_refused() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("real")).unwrap();
    let ln = d.path().join("out");
    std::os::unix::fs::symlink(d.path().join("real"), &ln).unwrap();
    assert!(output_hash(&ln).is_err());
    let mut j = job(d.path(), "true", Duration::from_secs(5));
    j.output_dir = ln;
    // The box made a real folder, so a link there is the payload's doing: void, no retry.
    let o = Level0.run_row(&j, &|| true).unwrap();
    assert!(
        matches!(&o.exit, Exit::Void(why) if why.starts_with("cannot hash output: ")),
        "{:?}",
        o.exit
    );
    assert_eq!(o.output_hash, "");
}

#[test]
fn an_output_the_payload_removed_is_void_unless_the_payload_failed() {
    let d = tempfile::tempdir().unwrap();
    let removed = |script: &str| {
        let j = job(d.path(), script, Duration::from_secs(5));
        Level0.run_row(&j, &|| true).unwrap()
    };
    let o = removed("rm -rf \"$OUT\"");
    assert_eq!(
        o.exit,
        Exit::Void("cannot hash output: No such file or directory (os error 2)".into())
    );
    assert_eq!(o.output_hash, "");
    // A failed payload stays a payload failure.
    assert_eq!(removed("rm -rf \"$OUT\"; exit 3").exit, Exit::Payload(3));
}

#[test]
fn payload_stdout_does_not_reach_the_box_stdout() {
    if std::env::var_os("LEVEL0_STDOUT_CHILD").is_some() {
        let d = tempfile::tempdir().unwrap();
        let o = Level0
            .run_row(
                &job(
                    d.path(),
                    "echo LEAKED; echo LEAKED >&2",
                    Duration::from_secs(30),
                ),
                &|| true,
            )
            .unwrap();
        assert_eq!(o.exit, Exit::Ok);
        return;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "payload_stdout_does_not_reach_the_box_stdout",
            "--nocapture",
        ])
        .env("LEVEL0_STDOUT_CHILD", "1")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(!String::from_utf8_lossy(&out.stdout).contains("LEAKED"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("LEAKED"));
}

#[test]
fn a_row_that_finishes_in_time_is_not_a_timeout() {
    let d = tempfile::tempdir().unwrap();
    let o = Level0
        .run_row(&job(d.path(), "exit 0", Duration::from_secs(30)), &|| true)
        .unwrap();
    assert_eq!(o.exit, Exit::Ok);
}

#[test]
fn level0_stops_the_group_when_the_watch_says_stop() {
    let d = tempfile::tempdir().unwrap();
    let ticks = Cell::new(0u32);
    // The 20th tick, about 100 ms in, says stop.
    let watch = || {
        ticks.set(ticks.get() + 1);
        ticks.get() < 20
    };
    let o = Level0
        .run_row(
            &job(
                d.path(),
                "(sleep 1; printf late > \"$OUT/late\") & sleep 30",
                Duration::from_secs(60),
            ),
            &watch,
        )
        .unwrap();
    assert_eq!(o.exit, Exit::Stopped);
    // A row the box ended is not data, so its output is not hashed.
    assert_eq!(o.output_hash, "");
    assert_eq!(ticks.get(), 20);
    assert!(o.wall_seconds < 2.0, "{}", o.wall_seconds);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(!d.path().join("out/late").exists());
}

#[test]
fn level0_ticks_the_watch_while_the_row_runs() {
    let d = tempfile::tempdir().unwrap();
    let ticks = Cell::new(0u32);
    let watch = || {
        ticks.set(ticks.get() + 1);
        true
    };
    let o = Level0
        .run_row(&job(d.path(), "sleep 0.3", Duration::from_secs(30)), &watch)
        .unwrap();
    assert_eq!(o.exit, Exit::Ok);
    // One tick per 5 ms poll: about 60 in 0.3 s.
    assert!(ticks.get() >= 10, "{}", ticks.get());
}

/// `f()`, or a panic when it takes over 5 s.
fn within<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(f()));
    rx.recv_timeout(Duration::from_secs(5))
        .expect("the call returns")
}

#[test]
fn waits_on_a_process_that_is_not_a_child_fail_at_once() {
    let me = std::process::id();
    assert!(within(move || sys::wait4(me, true)).is_err());
    assert!(within(move || sys::has_exited(me)).is_err());
}

#[test]
fn killing_a_group_that_does_not_exist_is_fine() {
    // Above the kernel's largest process id, so no group has it.
    assert!(sys::kill_group(i32::MAX as u32).is_ok());
}

/// Records each `started` call with the process group of the pid at that moment.
#[derive(Default)]
struct Leaders(std::cell::RefCell<Vec<(u32, u32)>>);

impl Watch for Leaders {
    fn tick(&self) -> bool {
        true
    }
    fn started(&self, pid: u32) {
        let pid_t = rustix::process::Pid::from_raw(pid as i32).unwrap();
        let group = rustix::process::getpgid(Some(pid_t)).unwrap();
        self.0
            .borrow_mut()
            .push((pid, group.as_raw_nonzero().get() as u32));
    }
}

#[test]
fn every_registered_backend_reports_one_leader_start() {
    for b in ignorarium::backend::registered() {
        if b.probe() != Probe::Available {
            println!("skip level{}: unavailable", b.level());
            continue;
        }
        let d = tempfile::tempdir().unwrap();
        let watch = Leaders::default();
        let o = b
            .run_row(&job(d.path(), "sleep 0.2", Duration::from_secs(30)), &watch)
            .unwrap();
        assert_eq!(o.exit, Exit::Ok);
        let seen = watch.0.borrow();
        assert_eq!(seen.len(), 1, "level {}", b.level());
        assert_eq!(seen[0].1, seen[0].0);
    }
}

#[test]
fn level0_payload_sees_its_host_paths() {
    let d = tempfile::tempdir().unwrap();
    let j = job(
        d.path(),
        "printf '%s|%s' \"$IGNORARIUM_IN\" \"$IGNORARIUM_OUT\" > \"$OUT/paths\"",
        Duration::from_secs(30),
    );
    let o = Level0.run_row(&j, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Ok);
    let seen = std::fs::read_to_string(j.output_dir.join("paths")).unwrap();
    assert_eq!(
        seen,
        format!("{}|{}", j.input_dir.display(), j.output_dir.display())
    );
}

#[test]
fn level0_names_its_launcher() {
    assert_eq!(Level0.launcher(), "process");
}
