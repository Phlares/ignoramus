mod common;

use common::{
    Lab, PAYLOAD, events_of, indices, kind_of, lab, named, plain, register_study, rows_of,
};
use ignorarium::backend::level0::Level0;
use ignorarium::backend::{Backend, Exit, Probe, RowJob, RowOutcome, Watch};
use ignorarium::budget::{Cost, FakeMeter, MeterError};
use ignorarium::canon::sha256_hex;
use ignorarium::control::State;
use ignorarium::planlog::store::PlanLog;
use ignorarium::prior::{PriorUse, file_hashes};
use ignorarium::runner::{RunError, RunPlan, RunStatus, prepare, prepare_with, run, up_to};
use ignorarium::{estimate, keys, records, schedule, spec};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

fn conf_input(t: String) -> String {
    t.replace("data/example.txt", "data/conf.txt")
        .replace("role = \"calibration\"", "role = \"confirmation\"")
}

/// Runs level 0, then replaces the exit with the next scripted one.
struct Fake {
    script: RefCell<VecDeque<Exit>>,
    seen: Rc<RefCell<Vec<u64>>>,
    probe: Probe,
    panic_at: Option<u64>,
    level: u8,
    wall: Option<f64>,
}

impl Backend for Fake {
    fn level(&self) -> u8 {
        self.level
    }
    fn probe(&self) -> Probe {
        self.probe.clone()
    }
    fn run_row(&self, job: &RowJob, watch: &dyn Watch) -> std::io::Result<RowOutcome> {
        let env = |k: &str| job.env.iter().find(|(n, _)| n == k).unwrap().1.clone();
        let index: u64 = env("IGNORARIUM_ROW").parse().unwrap();
        self.seen.borrow_mut().push(index);
        let mut o = Level0.run_row(job, watch)?;
        if let Some(w) = self.wall {
            o.wall_seconds = w;
            o.cpu_seconds = w / 2.0;
        }
        if self.panic_at == Some(index) {
            panic!("crash after the sandbox exits");
        }
        Ok(match self.script.borrow_mut().pop_front() {
            Some(exit) => RowOutcome { exit, ..o },
            None => o,
        })
    }
}

fn fake(script: Vec<Exit>) -> (Fake, Rc<RefCell<Vec<u64>>>) {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let b = Fake {
        script: RefCell::new(script.into()),
        seen: seen.clone(),
        probe: Probe::Available,
        panic_at: None,
        level: 0,
        wall: None,
    };
    (b, seen)
}

fn plan_of(lab: &Lab, id: &str) -> RunPlan {
    prepare_with(&lab.home, id, vec![Box::new(Level0)]).unwrap()
}

fn err_of(lab: &Lab, id: &str) -> RunError {
    prepare_with(&lab.home, id, vec![Box::new(Level0)])
        .err()
        .expect("prepare should refuse")
}

#[test]
fn run_writes_one_record_per_row_in_order() {
    let lab = lab();
    let dir = plain(&lab, "s1");
    let plan = plan_of(&lab, "s1");
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let key = keys::load(&lab.home).unwrap().unwrap();
    let log = dir.join("records.jsonl");
    assert_eq!(records::verify(&log, &key.verifying_key()).unwrap(), 7);
    let rows = rows_of(&lab, "s1");
    assert_eq!(rows.len(), 6);
    for (i, r) in rows.iter().enumerate() {
        assert_eq!(r.row, Some(i as u64));
        assert_eq!(r.seq, i as u64 + 1);
        let row = &plan.schedule.rows[i];
        assert_eq!(r.body["condition"], row.condition.as_str());
        assert_eq!(r.body["unit"], row.unit.as_str());
        assert_eq!(r.body["repeat"], row.repeat);
        assert_eq!(r.body["seed"], row.seed_hex.as_str());
        assert_eq!(r.body["exit"], "ok");
        assert_eq!(r.body["attempts"], 1);
        assert_eq!(r.body["isolation"], 0);
        assert_eq!(r.body["payload_sha256"], plan.payload_sha256.as_str());
        assert_eq!(r.body["data_hashes"].as_array().unwrap().len(), 1);
        assert_eq!(r.body["output_hash"].as_str().unwrap().len(), 64);
        let out = std::fs::read_to_string(dir.join(format!("run/rows/{i}/out/result"))).unwrap();
        assert_eq!(
            out,
            format!("{} {} {}", row.condition, row.unit, row.repeat)
        );
    }
    // The schedule and every event name the registered spec hash.
    let hash = kind_of(&lab, "s1", "register")[0].body["spec_hash"].clone();
    let schedule: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("schedule.json")).unwrap()).unwrap();
    assert_eq!(schedule["spec_hash"], hash);
    let trace = &hash.as_str().unwrap()["sha256:".len()..][..32];
    let events = events_of(&dir);
    assert!(!events.is_empty());
    assert!(events.iter().all(|e| e["trace_id"] == trace));
    // A second run has nothing left to do.
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    assert_eq!(rows_of(&lab, "s1").len(), 6);
}

#[test]
fn resume_skips_only_recorded_rows() {
    let lab = lab();
    let dir = plain(&lab, "s2");
    run(&plan_of(&lab, "s2")).unwrap();
    // Keep the register record and rows 0..3.
    let log = dir.join("records.jsonl");
    let text = std::fs::read_to_string(&log).unwrap();
    let kept: String = text.split_inclusive('\n').take(4).collect();
    std::fs::write(&log, kept).unwrap();
    std::fs::rename(dir.join("run"), dir.join("run-old")).unwrap();
    let mut plan = plan_of(&lab, "s2");
    let (b, seen) = fake(vec![]);
    plan.backend = Box::new(b);
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    assert_eq!(*seen.borrow(), vec![3, 4, 5]);
    assert_eq!(indices(&lab, "s2"), vec![0, 1, 2, 3, 4, 5]);
}

#[test]
fn crash_before_record_reruns_the_row() {
    let lab = lab();
    let dir = plain(&lab, "s3");
    let mut plan = plan_of(&lab, "s3");
    let (mut b, _) = fake(vec![]);
    b.panic_at = Some(2);
    plan.backend = Box::new(b);
    let crashed = catch_unwind(AssertUnwindSafe(|| run(&plan)));
    assert!(crashed.is_err());
    assert_eq!(indices(&lab, "s3"), vec![0, 1]);
    let (b, seen) = fake(vec![]);
    plan.backend = Box::new(b);
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    assert_eq!(*seen.borrow(), vec![2, 3, 4, 5]);
    assert_eq!(indices(&lab, "s3"), vec![0, 1, 2, 3, 4, 5]);
    // The crashed attempt's output is kept under another name.
    assert!(dir.join("run/rows/2/out.crashed-0/result").is_file());
    assert!(dir.join("run/rows/2/out/result").is_file());
}

#[test]
fn wall_time_is_the_backend_monotonic_measure() {
    let lab = lab();
    register_study(&lab, "s4", |t| t, &[], "#!/bin/sh\nsleep 0.3\n");
    run(&plan_of(&lab, "s4")).unwrap();
    for r in rows_of(&lab, "s4") {
        let w = r.body["wall_seconds"].as_f64().unwrap();
        assert!((0.3..30.0).contains(&w), "{w}");
        assert!(r.body["cpu_seconds"].as_f64().unwrap() < w);
    }
}

#[test]
fn infra_failure_retries_then_voids_after_three() {
    let lab = lab();
    plain(&lab, "s5");
    let mut plan = plan_of(&lab, "s5");
    let infra = || Exit::Infra("box fault".into());
    let (b, seen) = fake(vec![infra(), infra(), infra(), infra()]);
    plan.backend = Box::new(b);
    let status = run(&plan).unwrap();
    assert!(
        matches!(&status, RunStatus::Paused(m) if m.contains("row 0")),
        "{status:?}"
    );
    assert_eq!(*seen.borrow(), vec![0, 0, 0, 0]);
    let rows = rows_of(&lab, "s5");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].body["exit"], "void:infra");
    assert_eq!(rows[0].body["attempts"], 4);
    // The three retried attempts each left a spend record.
    let spend = kind_of(&lab, "s5", "spend");
    assert_eq!(spend.len(), 3);
    assert!(
        spend
            .iter()
            .all(|s| s.row == Some(0) && s.body["reason"] == "infrastructure fault: box fault")
    );
}

#[test]
fn infra_failure_that_clears_is_retried_and_recorded_ok() {
    let lab = lab();
    plain(&lab, "s5b");
    let mut plan = plan_of(&lab, "s5b");
    let infra = || Exit::Infra("box fault".into());
    let (b, seen) = fake(vec![infra(), infra()]);
    plan.backend = Box::new(b);
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    assert_eq!(seen.borrow()[..4], [0, 0, 0, 1]);
    let rows = rows_of(&lab, "s5b");
    assert_eq!(rows[0].body["attempts"], 3);
    assert_eq!(rows[0].body["exit"], "ok");
    assert_eq!(rows.len(), 6);
}

#[test]
fn an_unavailable_backend_pauses_before_the_retry() {
    let lab = lab();
    plain(&lab, "s5c");
    let mut plan = plan_of(&lab, "s5c");
    let (mut b, seen) = fake(vec![Exit::Infra("x".into())]);
    b.probe = Probe::Unavailable("gone".into());
    plan.backend = Box::new(b);
    let status = run(&plan).unwrap();
    assert!(
        matches!(&status, RunStatus::Paused(m) if m.contains("gone")),
        "{status:?}"
    );
    assert_eq!(*seen.borrow(), vec![0]);
    assert!(rows_of(&lab, "s5c").is_empty());
}

/// A payload whose row 0 runs `edit`, a shell command, before it writes its result.
fn edits_in_row_0(edit: &str) -> String {
    format!(
        "#!/bin/sh\nif [ \"$IGNORARIUM_ROW\" = 0 ]; then {edit}; fi\n\
         echo ok > \"$IGNORARIUM_OUT/result\"\n"
    )
}

/// Run `id` and check that it pauses for `why` after row 0, with row 1 never started.
fn pauses_after_row_0(lab: &Lab, id: &str, why: &str) {
    let dir = lab.home.study_dir(id);
    let plan = plan_of(lab, id);
    assert_eq!(run(&plan).unwrap(), RunStatus::Paused(why.into()));
    assert_eq!(indices(lab, id), vec![0]);
    assert!(!dir.join("run/rows/1").exists());
    assert!(kind_of(lab, id, "spend").is_empty());
    assert_eq!(
        State::read(&lab.home, id).unwrap(),
        Some(State::Paused(why.into()))
    );
    let ev = events_of(&dir);
    let pause = named(&ev, "run.pause");
    assert_eq!(pause.len(), 1);
    assert_eq!(pause[0]["attrs"]["reason"], why);
    assert_eq!(named(&ev, "row.start").len(), 1);
    // The record names the bytes row 0 started with.
    let row = &rows_of(lab, id)[0];
    assert_eq!(row.body["payload_sha256"], plan.payload_sha256.as_str());
}

#[test]
fn a_payload_edited_mid_run_pauses_before_the_next_row() {
    let lab = lab();
    let edit = edits_in_row_0("printf '# edited\n' >> \"$0\"");
    common::with_payload(&lab, "edit-pay", &edit);
    pauses_after_row_0(&lab, "edit-pay", "payload/run changed after the run began");
}

#[test]
fn an_input_added_mid_run_pauses_before_the_next_row() {
    let lab = lab();
    // The one input, data/extra.txt, is absent until row 0 writes it.
    let extra = |t: String| t.replace("data/example.txt", "data/extra.txt");
    let edit = edits_in_row_0("printf x > \"$IGNORARIUM_IN/extra.txt\"");
    let dir = register_study(&lab, "add-input", extra, &[], &edit);
    std::fs::create_dir_all(dir.join("data")).unwrap();
    pauses_after_row_0(
        &lab,
        "add-input",
        "data/extra.txt changed after the run began",
    );
}

#[test]
fn a_payload_that_removes_its_output_runs_once_and_is_void() {
    let lab = lab();
    // Counts its runs beside the output folder, then removes the folder and exits 0.
    let payload =
        "#!/bin/sh\necho run >> \"$IGNORARIUM_OUT/../runs\"\nrm -rf \"$IGNORARIUM_OUT\"\n";
    let dir = common::with_payload(&lab, "gone", payload);
    assert_eq!(run(&plan_of(&lab, "gone")).unwrap(), RunStatus::Done);
    let rows = rows_of(&lab, "gone");
    assert_eq!(rows.len(), 6);
    for r in &rows {
        assert_eq!(r.body["exit"], "void:output");
        assert_eq!(r.body["attempts"], 1);
        assert_eq!(r.body["output_hash"], "");
        let reason = r.body["reason"].as_str().unwrap_or_default();
        assert!(reason.starts_with("cannot hash output: "), "{reason}");
    }
    let runs = std::fs::read_to_string(dir.join("run/rows/0/runs")).unwrap();
    assert_eq!(runs, "run\n");
    assert!(kind_of(&lab, "gone", "spend").is_empty());
}

#[test]
fn a_retry_that_would_reach_a_cap_pauses_instead() {
    let lab = lab();
    // Only the cpu cap can bind: the wall cap is 100 hours.
    let roomy = |t: String| t.replace("wall_hours = 1.0", "wall_hours = 100.0");
    common::edited(&lab, "retry-cap", roomy, PAYLOAD);
    let mut plan = plan_of(&lab, "retry-cap");
    // The failed attempt uses 2000 of the 3600 cpu seconds; a retry as costly would pass the cap.
    let (mut b, seen) = fake(vec![Exit::Infra("box fault".into())]);
    b.wall = Some(4000.0);
    plan.backend = Box::new(b);
    let why = "the next row would reach the cpu cap";
    assert_eq!(run(&plan).unwrap(), RunStatus::Paused(why.into()));
    assert_eq!(*seen.borrow(), vec![0]);
    assert!(rows_of(&lab, "retry-cap").is_empty());
    let spend = kind_of(&lab, "retry-cap", "spend");
    assert_eq!(spend.len(), 1);
    assert_eq!(spend[0].row, Some(0));
    assert_eq!(
        spend[0].body,
        serde_json::json!({"cpu_seconds": 2000.0, "wall_seconds": 4000.0,
                           "reason": "infrastructure fault: box fault"})
    );
    assert_eq!(plan.meter.sample().unwrap().cpu_seconds, 2000.0);
}

/// Runs level 0. Once row 0 has ended, this process gets SIGTERM, as from a supervisor,
/// and the backend returns only after the box has seen it, with `exit` in place of row 0's
/// exit when set. `calls` counts the attempts.
#[derive(Default)]
struct SignalAfterRow0 {
    exit: Option<Exit>,
    calls: Rc<std::cell::Cell<u32>>,
}

impl Backend for SignalAfterRow0 {
    fn level(&self) -> u8 {
        0
    }
    fn probe(&self) -> Probe {
        Probe::Available
    }
    fn run_row(&self, job: &RowJob, watch: &dyn Watch) -> std::io::Result<RowOutcome> {
        self.calls.set(self.calls.get() + 1);
        let mut o = Level0.run_row(job, watch)?;
        if job
            .env
            .iter()
            .any(|(k, v)| k == "IGNORARIUM_ROW" && v == "0")
        {
            if let Some(exit) = &self.exit {
                o.exit = exit.clone();
            }
            let me = std::process::id().to_string();
            let sent = std::process::Command::new("kill")
                .args(["-TERM", &me])
                .status()?;
            assert!(sent.success());
            let t = std::time::Instant::now();
            while !ignorarium::sys::interrupted() {
                assert!(t.elapsed().as_secs() < 5, "the signal never arrived");
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        Ok(o)
    }
}

#[test]
fn a_signal_between_rows_pauses_at_the_next_boundary() {
    // The signal flag is process-wide, so the test runs in a child process of its own.
    const NAME: &str = "a_signal_between_rows_pauses_at_the_next_boundary";
    if !common::is_child(NAME) {
        let here = std::env::current_dir().unwrap();
        assert!(common::in_child(NAME, &here, &[]));
        return;
    }
    ignorarium::sys::catch_interrupts().unwrap();
    let lab = lab();
    plain(&lab, "between");
    let mut plan = plan_of(&lab, "between");
    plan.backend = Box::new(SignalAfterRow0::default());
    let why = "interrupted by a signal";
    assert_eq!(run(&plan).unwrap(), RunStatus::Paused(why.into()));
    // Row 0 ended before the signal counted, so it has its record; row 1 never started.
    assert_eq!(indices(&lab, "between"), vec![0]);
    assert!(kind_of(&lab, "between", "spend").is_empty());
    assert!(!lab.home.study_dir("between").join("run/rows/1").exists());
}

#[test]
fn a_signal_during_a_failed_attempt_stops_the_retry() {
    const NAME: &str = "a_signal_during_a_failed_attempt_stops_the_retry";
    if !common::is_child(NAME) {
        let here = std::env::current_dir().unwrap();
        assert!(common::in_child(NAME, &here, &[]));
        return;
    }
    ignorarium::sys::catch_interrupts().unwrap();
    let lab = lab();
    plain(&lab, "no-retry");
    let mut plan = plan_of(&lab, "no-retry");
    let backend = SignalAfterRow0 {
        exit: Some(Exit::Infra("box fault".into())),
        ..Default::default()
    };
    let calls = backend.calls.clone();
    plan.backend = Box::new(backend);
    let why = "interrupted by a signal";
    assert_eq!(run(&plan).unwrap(), RunStatus::Paused(why.into()));
    // The failed attempt is spend; the retry never starts.
    assert_eq!(calls.get(), 1);
    assert!(rows_of(&lab, "no-retry").is_empty());
    let spend = kind_of(&lab, "no-retry", "spend");
    assert_eq!(spend.len(), 1);
    assert_eq!(spend[0].body["reason"], "infrastructure fault: box fault");
}

const REAPED: &str = "SIGCHLD is ignored; the box cannot see a row end";

/// Runs only as the child of the SIGCHLD test: a library run of a prepared study.
#[test]
fn sigchld_ignored_child() {
    let (Some(root), Some(id)) = (
        std::env::var_os("SIGCHLD_HOME"),
        std::env::var("SIGCHLD_STUDY").ok(),
    ) else {
        return;
    };
    let home = ignorarium::home::Home::open_at(std::path::Path::new(&root)).unwrap();
    let plan = prepare_with(&home, &id, vec![Box::new(Level0)]).unwrap();
    assert_eq!(run(&plan).unwrap(), RunStatus::Paused(REAPED.into()));
}

#[test]
fn a_library_run_with_sigchld_ignored_pauses_before_row_0() {
    let lab = lab();
    let dir = plain(&lab, "reaped");
    // The study is set up here; only the run happens in a process that ignores SIGCHLD.
    let out = std::process::Command::new("env")
        .arg("--ignore-signal=CHLD")
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", "sigchld_ignored_child"])
        .env("SIGCHLD_HOME", &lab.home.root)
        .env("SIGCHLD_STUDY", "reaped")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && text.contains(" 1 passed"), "{text}");
    assert!(rows_of(&lab, "reaped").is_empty());
    assert!(kind_of(&lab, "reaped", "spend").is_empty());
    assert!(!dir.join("run/rows").exists());
    assert_eq!(
        State::read(&lab.home, "reaped").unwrap(),
        Some(State::Paused(REAPED.into()))
    );
}

/// The signal mask `field` (`SigCgt`, `SigIgn`) of this process, from `/proc/self/status`.
fn signal_mask(field: &str) -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{field}:")))
        .unwrap();
    u64::from_str_radix(line.trim(), 16).unwrap()
}

#[test]
fn a_library_run_leaves_signal_handling_to_its_caller() {
    // Signal dispositions are process-wide, so the test runs in a child process of its own.
    const NAME: &str = "a_library_run_leaves_signal_handling_to_its_caller";
    if !common::is_child(NAME) {
        let here = std::env::current_dir().unwrap();
        assert!(common::in_child(NAME, &here, &[]));
        return;
    }
    let before = (signal_mask("SigCgt"), signal_mask("SigIgn"));
    let lab = lab();
    plain(&lab, "library");
    assert_eq!(run(&plan_of(&lab, "library")).unwrap(), RunStatus::Done);
    assert_eq!((signal_mask("SigCgt"), signal_mask("SigIgn")), before);
    // SIGINT (bit 1), SIGHUP (bit 0) and SIGTERM (bit 14) keep their default action.
    assert_eq!(signal_mask("SigCgt") & (1 << 1 | 1 << 0 | 1 << 14), 0);
}

#[test]
fn payload_failure_is_void_and_not_retried() {
    let lab = lab();
    plain(&lab, "s6");
    let mut plan = plan_of(&lab, "s6");
    let (b, seen) = fake(vec![Exit::Payload(1)]);
    plan.backend = Box::new(b);
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    assert_eq!(seen.borrow().iter().filter(|&&i| i == 0).count(), 1);
    let rows = rows_of(&lab, "s6");
    assert_eq!(rows[0].body["exit"], "payload:1");
    assert_eq!(rows[0].body["attempts"], 1);
    assert_eq!(rows.len(), 6);
}

#[test]
fn late_confirmation_data_used_before_is_refused() {
    let lab = lab();
    // Plan A and plan B both register with data/conf.txt absent.
    let a = register_study(&lab, "plan-a", conf_input, &[], PAYLOAD);
    let b = register_study(&lab, "plan-b", conf_input, &[], PAYLOAD);
    for dir in [&a, &b] {
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(dir.join("data/conf.txt"), "late file").unwrap();
    }
    // B runs first; its hashes land.
    plan_of(&lab, "plan-b");
    match err_of(&lab, "plan-a") {
        RunError::DataReused { path, label } => {
            assert_eq!(path, "data/conf.txt");
            assert_eq!(label, "study plan-b");
        }
        e => panic!("{e}"),
    }
    assert!(!a.join("schedule.json").exists());
}

#[test]
fn an_imported_phase_named_like_the_study_is_an_earlier_use() {
    let lab = lab();
    // An informal run of the same name used the confirmation file, then the study registers.
    let earlier = lab.root.join("conf.csv");
    std::fs::write(&earlier, "held-out rows").unwrap();
    assert_cmd::Command::cargo_bin("ignorarium")
        .unwrap()
        .env("IGNORARIUM_HOME", &lab.home.root)
        .args(["import-history", "--phase", "mnist-ablation"])
        .arg(&earlier)
        .assert()
        .code(0);
    let dir = register_study(&lab, "mnist-ablation", conf_input, &[], PAYLOAD);
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("data/conf.txt"), "held-out rows").unwrap();
    let err = err_of(&lab, "mnist-ablation");
    assert!(err.to_string().starts_with("E-CNF-005: "), "{err}");
    match err {
        RunError::DataReused { path, label } => {
            assert_eq!(path, "data/conf.txt");
            assert_eq!(label, "phase mnist-ablation");
        }
        e => panic!("{e}"),
    }
    // The imported leaf keeps the plain phase label.
    let (exact, norm) = file_hashes(b"held-out rows");
    let log = PlanLog::open(&lab.home.root.join("planlog")).unwrap();
    let uses = log.uses(&exact, &norm).unwrap();
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].1.label, "mnist-ablation");
}

#[test]
fn late_data_lands_in_the_plan_log_once() {
    let lab = lab();
    let dir = register_study(&lab, "late", conf_input, &[], PAYLOAD);
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("data/conf.txt"), "Late  File").unwrap();
    let (exact, norm) = file_hashes(b"Late  File");
    let log = PlanLog::open(&lab.home.root.join("planlog")).unwrap();
    assert_eq!(log.used(&exact, &norm).unwrap(), None);
    plan_of(&lab, "late");
    let hit = log.used(&exact, &norm).unwrap();
    assert!(hit.is_some_and(|h| h.contains("legacy")));
    let uses = log.uses(&exact, &norm).unwrap();
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].1.label, "study:late");
    // Resume runs prepare again: it neither refuses itself nor appends again.
    plan_of(&lab, "late");
    assert_eq!(log.uses(&exact, &norm).unwrap().len(), 1);
}

#[test]
fn data_present_at_registration_is_not_late() {
    let lab = lab();
    let dir = register_study(
        &lab,
        "early",
        conf_input,
        &[("data/conf.txt", "known")],
        PAYLOAD,
    );
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("data/conf.txt"), "known").unwrap();
    plan_of(&lab, "early");
    let (exact, norm) = file_hashes(b"known");
    let log = PlanLog::open(&lab.home.root.join("planlog")).unwrap();
    let uses = log.uses(&exact, &norm).unwrap();
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].1.kind, "plan");
}

#[test]
fn prepare_refuses_a_missing_backend_an_unknown_study_and_a_changed_spec() {
    let lab = lab();
    let dir = plain(&lab, "guard");
    let none = prepare_with(&lab.home, "guard", vec![]).err().unwrap();
    assert!(
        matches!(none, RunError::NoBackend { need: 0, .. }),
        "{none}"
    );
    let nobody = err_of(&lab, "nobody");
    assert!(matches!(nobody, RunError::NotRegistered(_)), "{nobody}");
    // A spec edited after registration no longer matches the register record.
    let spec_text = std::fs::read_to_string(dir.join("spec.toml")).unwrap();
    std::fs::rename(dir.join("spec.toml"), dir.join("spec.old")).unwrap();
    std::fs::write(
        dir.join("spec.toml"),
        spec_text.replace("repeats = 1", "repeats = 2"),
    )
    .unwrap();
    let err = err_of(&lab, "guard");
    assert!(matches!(err, RunError::SpecChanged), "{err}");
}

#[test]
fn a_pinned_payload_must_match() {
    let lab = lab();
    let pin = |t: String| t.replace("payload = \"\"", "payload = \"sha256:00\"");
    register_study(&lab, "pinned", pin, &[], PAYLOAD);
    let err = err_of(&lab, "pinned");
    assert!(matches!(err, RunError::PayloadChanged), "{err}");
}

#[test]
fn units_from_spec_matches_the_estimate_count() {
    let lab = lab();
    plain(&lab, "units");
    let s = plan_of(&lab, "units").spec;
    let units = schedule::units_from_spec(&s);
    assert_eq!(units.len() as u64, estimate::units(&s));
    assert_eq!(
        units[0],
        ("all/w0".to_string(), "all".to_string(), "genre".to_string())
    );
    let mut s2 = s.clone();
    s2.design.selection.clusters = vec!["b".into(), "a".into()];
    s2.design.confirmation.clusters = vec!["b".into(), "c".into()];
    let units = schedule::units_from_spec(&s2);
    assert_eq!(units.len() as u64, estimate::units(&s2));
    assert_eq!(units.len(), 9);
    assert_eq!(units[0].0, "a/w0");
    assert_eq!(units[8].0, "c/w2");
    assert_eq!(units[3].1, "b");
}

#[test]
fn two_crashed_attempts_keep_two_output_dirs() {
    let lab = lab();
    let dir = plain(&lab, "s3b");
    let mut plan = plan_of(&lab, "s3b");
    for _ in 0..2 {
        let (mut b, _) = fake(vec![]);
        b.panic_at = Some(0);
        plan.backend = Box::new(b);
        assert!(catch_unwind(AssertUnwindSafe(|| run(&plan))).is_err());
    }
    let (b, _) = fake(vec![]);
    plan.backend = Box::new(b);
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    for k in ["out.crashed-0", "out.crashed-1", "out"] {
        assert!(dir.join(format!("run/rows/0/{k}/result")).is_file(), "{k}");
    }
    assert!(!dir.join("run/rows/0/out.crashed-2").exists());
}

#[test]
fn the_wall_time_the_backend_reports_lands_in_the_record() {
    let lab = lab();
    plain(&lab, "wall");
    let mut plan = plan_of(&lab, "wall");
    let (mut b, _) = fake(vec![]);
    b.wall = Some(1.25);
    plan.backend = Box::new(b);
    run(&plan).unwrap();
    for r in rows_of(&lab, "wall") {
        assert_eq!(r.body["wall_seconds"], 1.25);
        assert_eq!(r.body["cpu_seconds"], 0.625);
    }
}

fn fake_at(level: u8) -> Box<dyn Backend> {
    let (mut b, _) = fake(vec![]);
    b.level = level;
    Box::new(b)
}

fn need_isolation(t: String) -> String {
    t.replace("min_isolation = 0", "min_isolation = 1")
}

#[test]
fn the_lowest_backend_that_meets_min_isolation_runs() {
    let lab = lab();
    plain(&lab, "lvl0");
    register_study(&lab, "lvl1", need_isolation, &[], PAYLOAD);
    let pick = |id: &str| {
        let all = vec![fake_at(2), fake_at(0), fake_at(1)];
        prepare_with(&lab.home, id, all).unwrap().backend.level()
    };
    assert_eq!(pick("lvl0"), 0);
    assert_eq!(pick("lvl1"), 1);
    let only0 = prepare_with(&lab.home, "lvl1", vec![fake_at(0)])
        .err()
        .unwrap();
    assert!(
        matches!(only0, RunError::NoBackend { need: 1, .. }),
        "{only0}"
    );
}

#[test]
fn backend_max_filters_out_higher_levels() {
    let levels = |max| -> Vec<u8> {
        up_to(vec![fake_at(0), fake_at(1), fake_at(2)], max)
            .iter()
            .map(|b| b.level())
            .collect()
    };
    assert_eq!(levels(0), vec![0]);
    assert_eq!(levels(1), vec![0, 1]);
    assert_eq!(levels(2), vec![0, 1, 2]);
    let lab = lab();
    plain(&lab, "reg");
    assert_eq!(prepare(&lab.home, "reg", 0).unwrap().backend.level(), 0);
    register_study(&lab, "reg1", need_isolation, &[], PAYLOAD);
    let err = prepare(&lab.home, "reg1", 0).err().unwrap();
    assert!(matches!(err, RunError::NoBackend { need: 1, .. }), "{err}");
}

#[test]
fn a_matching_payload_pin_runs_with_or_without_the_prefix() {
    let hash = sha256_hex(PAYLOAD.as_bytes());
    for (id, pin) in [("pin-a", format!("sha256:{hash}")), ("pin-b", hash.clone())] {
        let lab = lab();
        let set = move |t: String| t.replace("payload = \"\"", &format!("payload = \"{pin}\""));
        register_study(&lab, id, set, &[], PAYLOAD);
        assert_eq!(plan_of(&lab, id).payload_sha256, hash);
    }
}

#[test]
fn a_symlinked_payload_is_refused() {
    let lab = lab();
    let dir = plain(&lab, "link-payload");
    let outside = lab.root.join("outside-payload");
    std::fs::write(&outside, PAYLOAD).unwrap();
    std::fs::rename(dir.join("payload/run"), dir.join("payload/run.old")).unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("payload/run")).unwrap();
    assert!(matches!(err_of(&lab, "link-payload"), RunError::Unsafe(_)));
}

#[test]
fn a_symlinked_payload_folder_is_refused() {
    let lab = lab();
    let dir = plain(&lab, "link-pdir");
    let outside = lab.root.join("outside-pdir");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("run"), PAYLOAD).unwrap();
    std::fs::rename(dir.join("payload"), dir.join("payload.old")).unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("payload")).unwrap();
    assert!(matches!(err_of(&lab, "link-pdir"), RunError::Unsafe(_)));
}

/// Make `dir/<linked>` a link to an outside folder; `run` must refuse and touch nothing there.
fn run_is_refused_when_linked(id: &str, linked: &str) {
    let lab = lab();
    let dir = plain(&lab, id);
    let plan = plan_of(&lab, id);
    let outside = lab.root.join("outside-run");
    std::fs::create_dir(&outside).unwrap();
    let at = dir.join(linked);
    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &at).unwrap();
    let err = run(&plan).expect_err("run should refuse");
    assert!(matches!(err, RunError::Unsafe(_)), "{err}");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    assert!(rows_of(&lab, id).is_empty());
}

#[test]
fn a_symlinked_run_folder_is_refused() {
    run_is_refused_when_linked("link-run", "run");
}

#[test]
fn a_symlinked_rows_folder_is_refused() {
    run_is_refused_when_linked("link-rows", "run/rows");
}

#[test]
fn a_symlinked_row_folder_is_refused() {
    run_is_refused_when_linked("link-row", "run/rows/0");
}

#[test]
fn a_register_record_without_a_leaf_is_refused() {
    let lab = lab();
    let dir = plain(&lab, "noleaf");
    let key = keys::load(&lab.home).unwrap().unwrap();
    let log = dir.join("records.jsonl");
    std::fs::rename(&log, dir.join("records.old")).unwrap();
    let hash = spec::load(&dir.join("spec.toml")).unwrap().hash;
    let draft = records::RecordDraft {
        study: "noleaf".into(),
        row: None,
        kind: "register".into(),
        body: serde_json::json!({"spec_hash": hash, "log": "planlog"}),
    };
    records::append(&log, &key, draft).unwrap();
    let err = err_of(&lab, "noleaf");
    assert!(matches!(err, RunError::NotRegistered(_)), "{err}");
}

#[test]
fn a_reused_input_names_a_registered_plan_by_leaf() {
    let lab = lab();
    let a = register_study(&lab, "plan-a", conf_input, &[], PAYLOAD);
    register_study(
        &lab,
        "plan-c",
        conf_input,
        &[("data/conf.txt", "shared")],
        PAYLOAD,
    );
    std::fs::create_dir_all(a.join("data")).unwrap();
    std::fs::write(a.join("data/conf.txt"), "shared").unwrap();
    match err_of(&lab, "plan-a") {
        RunError::DataReused { label, .. } => assert_eq!(label, "registered plan, leaf 1"),
        e => panic!("{e}"),
    }
}

#[test]
fn a_plan_leaf_labelled_like_the_study_is_not_its_own_use() {
    let lab = lab();
    // A plan leaf carries the earnable label `signal`; a study with that id must not match it.
    let s = register_study(&lab, "signal", conf_input, &[], PAYLOAD);
    register_study(
        &lab,
        "other",
        conf_input,
        &[("data/conf.txt", "shared")],
        PAYLOAD,
    );
    std::fs::create_dir_all(s.join("data")).unwrap();
    std::fs::write(s.join("data/conf.txt"), "shared").unwrap();
    assert!(matches!(
        err_of(&lab, "signal"),
        RunError::DataReused { .. }
    ));
}

fn spent(cpu_seconds: f64, wall_seconds: f64, usd: f64) -> Result<Cost, MeterError> {
    Ok(Cost {
        cpu_seconds,
        wall_seconds,
        usd,
    })
}

fn cost(cpu: f64, wall: f64) -> Result<Cost, MeterError> {
    spent(cpu, wall, 0.0)
}

#[test]
fn planted_overspend_leaves_the_crossing_row_unrun() {
    let lab = lab();
    let dir = plain(&lab, "b1");
    let mut plan = plan_of(&lab, "b1");
    // Row 0 starts from zero. Row 1 sees 3570 of the 3600 cpu seconds; a row estimates 60.
    plan.meter = Box::new(FakeMeter::new(vec![cost(0.0, 0.0), cost(3570.0, 0.0)]));
    let (b, seen) = fake(vec![]);
    plan.backend = Box::new(b);
    let status = run(&plan).unwrap();
    assert_eq!(
        status,
        RunStatus::Paused("the next row would reach the cpu cap".into())
    );
    assert_eq!(*seen.borrow(), vec![0]);
    assert_eq!(indices(&lab, "b1"), vec![0]);
    assert!(dir.join("run/rows/0/out").is_dir());
    assert!(!dir.join("run/rows/1").exists());
    let ev = events_of(&dir);
    let pause = named(&ev, "run.pause");
    assert_eq!(pause.len(), 1);
    assert_eq!(
        pause[0]["attrs"]["reason"],
        "the next row would reach the cpu cap"
    );
    assert_eq!(pause[0]["level"], "warn");
    // 3570 of 3600 is past 95 %, but the first sample seeds the alerts; the second raises all three.
    let alerts = named(&ev, "budget.alert");
    let fractions: Vec<f64> = alerts
        .iter()
        .map(|e| e["attrs"]["fraction"].as_f64().unwrap())
        .collect();
    assert_eq!(fractions, vec![0.5, 0.8, 0.95]);
    assert_eq!(alerts[0]["attrs"]["dimension"], "cpu");
    // The alerts come before the pause.
    let pos = |n: &str| ev.iter().position(|e| e["name"] == n).unwrap();
    assert!(pos("budget.alert") < pos("run.pause"));
    assert_eq!(pause[0]["trace_id"], alerts[0]["trace_id"]);
}

#[test]
fn a_reached_cap_pauses_before_the_first_row() {
    let lab = lab();
    let dir = plain(&lab, "b2");
    let mut plan = plan_of(&lab, "b2");
    plan.meter = Box::new(FakeMeter::new(vec![cost(0.0, 3600.0)]));
    assert_eq!(
        run(&plan).unwrap(),
        RunStatus::Paused("the wall cap is reached".into())
    );
    assert!(indices(&lab, "b2").is_empty());
    assert!(!dir.join("run/rows").exists());
}

#[test]
fn meter_fault_pauses_the_run_before_any_row() {
    let lab = lab();
    let dir = plain(&lab, "b3");
    let mut plan = plan_of(&lab, "b3");
    plan.meter = Box::new(FakeMeter::new(vec![Err(MeterError("no reading".into()))]));
    assert_eq!(
        run(&plan).unwrap(),
        RunStatus::Paused("the cost meter failed: no reading".into())
    );
    assert!(indices(&lab, "b3").is_empty());
    assert!(!dir.join("run/rows").exists());
    assert_eq!(named(&events_of(&dir), "run.pause").len(), 1);
}

#[test]
fn a_meter_fault_after_some_rows_pauses_there() {
    let lab = lab();
    plain(&lab, "b3b");
    let mut plan = plan_of(&lab, "b3b");
    plan.meter = Box::new(FakeMeter::new(vec![
        cost(0.0, 0.0),
        cost(0.0, 0.0),
        Err(MeterError("gone".into())),
    ]));
    assert!(matches!(run(&plan).unwrap(), RunStatus::Paused(_)));
    assert_eq!(indices(&lab, "b3b"), vec![0, 1]);
}

#[test]
fn a_fresh_study_whose_first_reading_is_high_still_emits_its_alerts() {
    let lab = lab();
    let dir = plain(&lab, "b4");
    let mut plan = plan_of(&lab, "b4");
    // 0.85 of the wall cap at the first reading; a row estimates 60 s, so the run goes on.
    plan.meter = Box::new(FakeMeter::new(vec![cost(0.0, 3060.0)]));
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let ev = events_of(&dir);
    let alerts = named(&ev, "budget.alert");
    let fractions: Vec<f64> = alerts
        .iter()
        .map(|e| e["attrs"]["fraction"].as_f64().unwrap())
        .collect();
    assert_eq!(fractions, vec![0.5, 0.8]);
    assert!(alerts.iter().all(|e| e["attrs"]["dimension"] == "wall"));
    assert!(named(&ev, "run.pause").is_empty());
}

#[test]
fn alerts_crossed_by_earlier_sessions_stay_silent_on_resume() {
    let lab = lab();
    let dir = plain(&lab, "b4b");
    let mut plan = plan_of(&lab, "b4b");
    let slow = || {
        let (mut b, _) = fake(vec![]);
        b.wall = Some(1000.0);
        Box::new(b)
    };
    plan.backend = slow();
    plan.meter = Box::new(FakeMeter::new(vec![cost(0.0, 0.0)]));
    run(&plan).unwrap();
    // Keep the register record and rows 0 and 1: 2000 s of 3600 s, past 0.50.
    let log = dir.join("records.jsonl");
    let text = std::fs::read_to_string(&log).unwrap();
    let kept: String = text.split_inclusive('\n').take(3).collect();
    std::fs::write(&log, kept).unwrap();
    let mut plan = plan_of(&lab, "b4b");
    plan.backend = slow();
    plan.meter = Box::new(FakeMeter::new(vec![cost(0.0, 2000.0), cost(0.0, 3000.0)]));
    let status = run(&plan).unwrap();
    assert_eq!(
        status,
        RunStatus::Paused("the next row would reach the wall cap".into())
    );
    let ev = events_of(&dir);
    let fractions: Vec<f64> = named(&ev, "budget.alert")
        .iter()
        .map(|e| e["attrs"]["fraction"].as_f64().unwrap())
        .collect();
    assert_eq!(fractions, vec![0.8]);
}

#[test]
fn retried_attempts_count_in_the_record_the_tally_and_the_meter() {
    let lab = lab();
    plain(&lab, "b7");
    let mut plan = plan_of(&lab, "b7");
    let infra = || Exit::Infra("box fault".into());
    let (mut b, _) = fake(vec![infra(), infra()]);
    b.wall = Some(100.0);
    plan.backend = Box::new(b);
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    // The row record holds the last attempt; each failed attempt has a spend record.
    let rows = rows_of(&lab, "b7");
    assert_eq!(rows[0].body["attempts"], 3);
    assert_eq!(rows[0].body["wall_seconds"], 100.0);
    assert_eq!(rows[0].body["cpu_seconds"], 50.0);
    assert_eq!(rows[1].body["wall_seconds"], 100.0);
    let spend = kind_of(&lab, "b7", "spend");
    assert_eq!(spend.len(), 2);
    for s in &spend {
        assert_eq!(s.row, Some(0));
        assert_eq!(s.body["cpu_seconds"], 50.0);
        assert_eq!(s.body["wall_seconds"], 100.0);
    }
    // Rows 1..5 cost 50 cpu seconds each; row 0 cost 3 attempts of 50 -> 400 in all.
    assert_eq!(plan.meter.sample().unwrap().cpu_seconds, 400.0);
    // A resume reads the same 400 from the records: no attempt counts twice.
    let resumed = plan_of(&lab, "b7");
    assert_eq!(run(&resumed).unwrap(), RunStatus::Done);
    assert_eq!(resumed.meter.sample().unwrap().cpu_seconds, 400.0);
}

#[test]
fn spend_survives_a_resume_through_the_signed_records() {
    let lab = lab();
    let dir = plain(&lab, "b5");
    let mut plan = plan_of(&lab, "b5");
    let (mut b, _) = fake(vec![]);
    b.wall = Some(100.0);
    plan.backend = Box::new(b);
    run(&plan).unwrap();
    // Keep the register record and rows 0..3, as a crash would.
    let log = dir.join("records.jsonl");
    let text = std::fs::read_to_string(&log).unwrap();
    let kept: String = text.split_inclusive('\n').take(4).collect();
    std::fs::write(&log, kept).unwrap();
    let mut resumed = plan_of(&lab, "b5");
    let (mut b, _) = fake(vec![]);
    b.wall = Some(100.0);
    resumed.backend = Box::new(b);
    run(&resumed).unwrap();
    // 150 cpu seconds from the 3 kept rows, 150 from the 3 rows this run.
    let c = resumed.meter.sample().unwrap();
    assert_eq!(c.cpu_seconds, 300.0);
    assert!(
        (300.0..330.0).contains(&c.wall_seconds),
        "{}",
        c.wall_seconds
    );
}

#[test]
fn the_meter_starts_from_the_spend_read_under_the_run_lock() {
    let lab = lab();
    let dir = plain(&lab, "locked");
    let plan = plan_of(&lab, "locked");
    // Another run appends 3000 cpu seconds of spend after this plan was prepared.
    let key = keys::load(&lab.home).unwrap().unwrap();
    let draft = records::RecordDraft {
        study: "locked".into(),
        row: Some(0),
        kind: "spend".into(),
        body: serde_json::json!({"cpu_seconds": 3000.0, "wall_seconds": 0.0, "reason": "x"}),
    };
    records::append(&dir.join("records.jsonl"), &key, draft).unwrap();
    // 3000 spent and a 3000 s mean row reach the 3600 s cap.
    assert_eq!(
        run(&plan).unwrap(),
        RunStatus::Paused("the next row would reach the cpu cap".into())
    );
    assert!(rows_of(&lab, "locked").is_empty());
    assert_eq!(plan.meter.sample().unwrap().cpu_seconds, 3000.0);
}

#[test]
fn the_next_row_costs_the_larger_of_the_estimate_and_the_recorded_mean() {
    let lab = lab();
    plain(&lab, "b6");
    let mut plan = plan_of(&lab, "b6");
    // Rows take 1000 s of wall, far over the 60 s estimate. After three rows 3000 s are spent
    // and the mean says the fourth reaches 4000 s, past the 3600 s cap.
    plan.meter = Box::new(FakeMeter::new(vec![
        cost(0.0, 0.0),
        cost(0.0, 1000.0),
        cost(0.0, 2000.0),
        cost(0.0, 3000.0),
    ]));
    let (mut b, seen) = fake(vec![]);
    b.wall = Some(1000.0);
    plan.backend = Box::new(b);
    assert_eq!(
        run(&plan).unwrap(),
        RunStatus::Paused("the next row would reach the wall cap".into())
    );
    assert_eq!(*seen.borrow(), vec![0, 1, 2]);
}

#[test]
fn the_recorded_mean_counts_cpu_as_well_as_wall() {
    let lab = lab();
    plain(&lab, "b6c");
    let mut plan = plan_of(&lab, "b6c");
    // Rows take 500 cpu seconds. After two rows the meter reads 3200 s, and the mean says
    // the third reaches 3700 s, past the 3600 s cap.
    plan.meter = Box::new(FakeMeter::new(vec![
        cost(0.0, 0.0),
        cost(500.0, 0.0),
        cost(3200.0, 0.0),
    ]));
    let (mut b, seen) = fake(vec![]);
    b.wall = Some(1000.0);
    plan.backend = Box::new(b);
    assert_eq!(
        run(&plan).unwrap(),
        RunStatus::Paused("the next row would reach the cpu cap".into())
    );
    assert_eq!(*seen.borrow(), vec![0, 1]);
}

/// `plain` with a 10 USD cap, on a machine that charges 36 USD an hour. A row then
/// estimates 60 cpu seconds, 60 wall seconds and 0.60 USD.
fn priced(lab: &Lab, id: &str) {
    let edit = |t: String| t.replace("money_usd = 0.0", "money_usd = 10.0");
    common::edited(lab, id, edit, PAYLOAD);
    std::fs::write(
        lab.home.root.join("machine.toml"),
        "max_level = 1\nrate_usd_per_hour = 36.0\n",
    )
    .unwrap();
}

#[test]
fn the_first_row_is_held_to_its_share_of_the_spec_estimate() {
    let lab = lab();
    // Each dimension just under and just over its cap minus one row's estimate.
    let cases = [
        ("cpu-room", spent(3500.0, 0.0, 0.0), None),
        ("cpu-full", spent(3560.0, 0.0, 0.0), Some("cpu")),
        ("wall-room", spent(0.0, 3500.0, 0.0), None),
        ("wall-full", spent(0.0, 3560.0, 0.0), Some("wall")),
        ("usd-room", spent(0.0, 0.0, 9.3), None),
        ("usd-full", spent(0.0, 0.0, 9.5), Some("money")),
    ];
    for (id, reading, cap) in cases {
        priced(&lab, id);
        let mut plan = plan_of(&lab, id);
        plan.meter = Box::new(FakeMeter::new(vec![reading]));
        let want = match cap {
            None => RunStatus::Done,
            Some(d) => RunStatus::Paused(format!("the next row would reach the {d} cap")),
        };
        assert_eq!(run(&plan).unwrap(), want, "{id}");
    }
}

#[test]
fn the_recorded_mean_prices_wall_time_at_the_machine_rate() {
    let lab = lab();
    priced(&lab, "usd-mean");
    let mut plan = plan_of(&lab, "usd-mean");
    // Rows take 100 s of wall: 1.00 USD each at 36 USD an hour, over the 0.60 estimate.
    // Before row 2, 7.00 + 1.00 fits the 10 USD cap; before row 3, 9.20 + 1.00 does not.
    plan.meter = Box::new(FakeMeter::new(vec![
        spent(0.0, 0.0, 0.0),
        spent(0.0, 0.0, 1.0),
        spent(0.0, 0.0, 7.0),
        spent(0.0, 0.0, 9.2),
    ]));
    let (mut b, seen) = fake(vec![]);
    b.wall = Some(100.0);
    plan.backend = Box::new(b);
    assert_eq!(
        run(&plan).unwrap(),
        RunStatus::Paused("the next row would reach the money cap".into())
    );
    assert_eq!(*seen.borrow(), vec![0, 1, 2]);
}

#[test]
fn an_unrequested_backend_stop_voids_the_row_and_pauses() {
    let lab = lab();
    plain(&lab, "void-stop");
    let mut plan = plan_of(&lab, "void-stop");
    let (b, seen) = fake(vec![Exit::Stopped]);
    plan.backend = Box::new(b);
    assert_eq!(
        run(&plan).unwrap(),
        RunStatus::Paused(
            "row 0 voided by infrastructure faults: the backend stopped the row with no stop requested"
                .into()
        )
    );
    // A backend fault, not a payload result: no retry, and the run pauses.
    assert_eq!(*seen.borrow(), vec![0]);
    let rows = rows_of(&lab, "void-stop");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].body["exit"], "void:infra");
    assert_eq!(rows[0].body["attempts"], 1);
}

#[test]
fn attempts_before_a_backend_pause_are_spend_that_counts_on_resume() {
    // 100 s of a 3600 s wall cap leaves room; 3000 s means the next row would reach it.
    for (id, wall) in [("spend-low", 100.0), ("spend-high", 3000.0)] {
        let lab = lab();
        plain(&lab, id);
        let mut plan = plan_of(&lab, id);
        let (mut b, _) = fake(vec![Exit::Infra("box fault".into())]);
        b.probe = Probe::Unavailable("gone".into());
        b.wall = Some(wall);
        plan.backend = Box::new(b);
        assert_eq!(
            run(&plan).unwrap(),
            RunStatus::Paused("backend unavailable: gone".into())
        );
        assert!(rows_of(&lab, id).is_empty());
        let spend = kind_of(&lab, id, "spend");
        assert_eq!(spend.len(), 1);
        assert_eq!(spend[0].row, Some(0));
        assert_eq!(
            spend[0].body,
            serde_json::json!({"cpu_seconds": wall / 2.0, "wall_seconds": wall,
                               "reason": "infrastructure fault: box fault"})
        );
        // The signed spend record is part of the earlier spend a resume starts from.
        let resumed = plan_of(&lab, id);
        let status = run(&resumed).unwrap();
        let c = resumed.meter.sample().unwrap();
        assert!(c.wall_seconds >= wall, "{}", c.wall_seconds);
        if wall < 1000.0 {
            assert_eq!(status, RunStatus::Done);
            assert_eq!(rows_of(&lab, id).len(), 6);
            assert!(c.cpu_seconds >= wall / 2.0, "{}", c.cpu_seconds);
        } else {
            assert_eq!(
                status,
                RunStatus::Paused("the next row would reach the wall cap".into())
            );
            assert!(rows_of(&lab, id).is_empty());
            assert_eq!(c.cpu_seconds, wall / 2.0);
        }
    }
}
