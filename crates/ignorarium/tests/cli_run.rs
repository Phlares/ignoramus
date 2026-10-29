//! The run verbs, driven through the real binary. The test machine file reaches level 1,
//! so rows run in bubblewrap.
mod common;
use assert_cmd::Command;
use common::{Lab, kind_of, lab, no_level1, plain, register_study, with_payload};
use ignorarium::cli::status::blind_order;
use ignorarium::control::{ControlFile, RunLock, Signal, State};
use ignorarium::keys;
use serde_json::Value;
use std::time::Duration;

fn bin(lab: &Lab) -> Command {
    let mut c = Command::cargo_bin("ignorarium").unwrap();
    c.env("IGNORARIUM_HOME", &lab.home.root);
    c
}

fn text(a: &assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&a.get_output().stdout).into_owned()
}

fn err(a: &assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&a.get_output().stderr).into_owned()
}

fn status(lab: &Lab, id: &str) -> Value {
    let a = bin(lab).args(["status", id, "--json"]).assert().success();
    serde_json::from_str(&text(&a)).unwrap()
}

#[test]
fn probe_writes_machine_toml_and_prints_the_level() {
    let lab = lab();
    std::fs::write(
        lab.home.root.join("machine.toml"),
        "max_level = 0\nrate_usd_per_hour = 2.5\n",
    )
    .unwrap();
    // Level 1 where bubblewrap can run here, else level 0.
    let want = u8::from(no_level1().is_none());
    let a = bin(&lab).arg("probe").assert().code(0);
    let out = text(&a);
    assert!(out.contains(&format!("max level: {want}")), "{out}");
    assert!(out.contains("/dev/kvm"), "{out}");
    assert!(out.contains("level 0 (process): available\n"), "{out}");
    if want == 1 {
        assert!(out.contains("): available\n"), "{out}");
        assert!(out.contains("level 1 (bubblewrap 0."), "{out}");
    }
    let m = ignorarium::machine::load(&lab.home.root).unwrap();
    assert_eq!(m.max_level, want);
    assert_eq!(m.rate_usd_per_hour, 2.5);
    let events = common::events_of(&lab.home.root);
    let probe = common::named(&events, "probe");
    assert_eq!(probe.len(), 1);
    assert_eq!(probe[0]["attrs"]["max_level"], want);
    assert!(probe[0]["attrs"]["kvm"].is_boolean());
}

#[test]
fn probe_names_the_reason_each_backend_is_unavailable() {
    let lab = lab();
    let dir = tempfile::tempdir().unwrap();
    // No bwrap on PATH: the reason is the one the backend gives, verbatim.
    let a = bin(&lab)
        .env("PATH", dir.path())
        .arg("probe")
        .assert()
        .code(0);
    let out = text(&a);
    assert!(out.contains("level 0 (process): available\n"), "{out}");
    assert!(
        out.contains("level 1 (bubblewrap unknown): unavailable: cannot run bwrap: "),
        "{out}"
    );
    assert!(out.contains("max level: 0\n"), "{out}");
    // A bwrap older than the floor.
    let old = dir.path().join("bwrap");
    common::write_exec(&old, "#!/bin/sh\necho 'bubblewrap 0.7.0'\n");
    let a = bin(&lab)
        .env("PATH", dir.path())
        .arg("probe")
        .assert()
        .code(0);
    let out = text(&a);
    let want = "level 1 (bubblewrap 0.7.0): unavailable: bubblewrap 0.7.0 is older than 0.8, \
                which level 1 needs for --disable-userns\n";
    assert!(out.contains(want), "{out}");
}

#[test]
fn probe_refuses_a_bad_machine_file() {
    let lab = lab();
    std::fs::write(lab.home.root.join("machine.toml"), "cpus = 0\n").unwrap();
    let a = bin(&lab).arg("probe").assert().code(1);
    assert!(err(&a).contains("machine.toml"), "{}", err(&a));
}

#[test]
fn run_a_registered_study_to_completion() {
    let lab = lab();
    plain(&lab, "s1");
    let a = bin(&lab).args(["run", "s1"]).assert().code(0);
    assert!(text(&a).contains("done"), "{}", text(&a));
    let s = status(&lab, "s1");
    assert_eq!(s["state"], "done");
    assert!(s["total"].as_u64().unwrap() > 0);
    assert_eq!(s["done"], s["total"]);
    assert_eq!(kind_of(&lab, "s1", "row").len() as u64, s["total"]);
    // One trace for the whole study; each row has its own span.
    let events = common::events_of(&lab.home.study_dir("s1"));
    let traces: std::collections::BTreeSet<_> = events
        .iter()
        .map(|e| e["trace_id"].as_str().unwrap())
        .collect();
    assert_eq!(traces.len(), 1);
    assert_eq!(common::named(&events, "row.start").len() as u64, s["total"]);
    // The verb's own event names the spec it ran.
    let hash = ignorarium::spec::load(&lab.home.study_dir("s1").join("spec.toml"))
        .unwrap()
        .hash;
    let calls = common::events_of(&lab.home.root);
    let run = common::named(&calls, "run");
    assert_eq!(run.len(), 1);
    assert_eq!(run[0]["attrs"]["spec_hash"], hash.as_str());
}

#[test]
fn run_refuses_when_the_machine_is_below_min_isolation() {
    let lab = lab();
    std::fs::write(lab.home.root.join("machine.toml"), "max_level = 2\n").unwrap();
    register_study(
        &lab,
        "hi",
        |t| t.replace("min_isolation = 0", "min_isolation = 2"),
        &[],
        common::PAYLOAD,
    );
    std::fs::write(lab.home.root.join("machine.toml"), "max_level = 0\n").unwrap();
    let a = bin(&lab).args(["run", "hi"]).assert().code(1);
    let e = err(&a);
    assert!(e.contains("level 2"), "{e}");
    assert!(e.contains("ignorarium probe"), "{e}");
    assert!(kind_of(&lab, "hi", "row").is_empty());
}

#[test]
fn max_level_flag_parses_and_a_value_above_3_is_a_usage_error() {
    let lab = lab();
    plain(&lab, "ml");
    bin(&lab)
        .args(["run", "ml", "--max-level", "4"])
        .assert()
        .code(2);
    assert!(kind_of(&lab, "ml", "row").is_empty());
    bin(&lab)
        .args(["run", "ml", "--max-level", "0"])
        .assert()
        .code(0);
    assert_eq!(status(&lab, "ml")["state"], "done");
}

#[test]
fn run_refuses_an_unknown_study_and_a_bad_id() {
    let lab = lab();
    let a = bin(&lab).args(["run", "nope"]).assert().code(1);
    assert!(err(&a).contains("nope"), "{}", err(&a));
    bin(&lab).args(["status", "nope"]).assert().code(1);
    bin(&lab).args(["pause", "nope"]).assert().code(1);
    bin(&lab).args(["resume", "nope"]).assert().code(1);
    bin(&lab)
        .args(["stop", "nope", "--reason", "x"])
        .assert()
        .code(1);
    let a = bin(&lab).args(["status", "../x"]).assert().code(1);
    assert!(err(&a).contains("../x"), "{}", err(&a));
    assert!(!lab.home.study_dir("nope").exists());
}

#[test]
fn pause_then_status_shows_paused_and_run_refuses_until_resume() {
    let lab = lab();
    plain(&lab, "p1");
    bin(&lab).args(["pause", "p1"]).assert().code(0);
    let s = status(&lab, "p1");
    assert_eq!(s["request"], "pause");
    let a = bin(&lab).args(["run", "p1"]).assert().code(1);
    assert!(
        err(&a).contains("a pause is pending; run `ignorarium resume p1`"),
        "{}",
        err(&a)
    );
    assert!(kind_of(&lab, "p1", "row").is_empty());
    // resume clears the pause and runs to the end.
    bin(&lab).args(["resume", "p1"]).assert().code(0);
    let s = status(&lab, "p1");
    assert_eq!(s["state"], "done");
    assert!(s["request"].is_null());
}

#[test]
fn a_run_stopped_by_the_budget_exits_1_with_the_reason() {
    let lab = lab();
    register_study(
        &lab,
        "bud",
        |t| t.replace("cpu_hours = 1.0", "cpu_hours = 0.000001"),
        &[],
        common::PAYLOAD,
    );
    let a = bin(&lab).args(["run", "bud"]).assert().code(1);
    assert!(
        err(&a).contains("paused: the next row would reach the cpu cap"),
        "{}",
        err(&a)
    );
    let s = status(&lab, "bud");
    assert_eq!(s["state"], "paused");
    assert_eq!(s["detail"], "the next row would reach the cpu cap");
    assert_eq!(s["done"], 0);
}

#[test]
fn stop_writes_a_record_and_status_shows_stopped() {
    let lab = lab();
    plain(&lab, "x1");
    let a = bin(&lab)
        .args(["stop", "x1", "--reason", "enough"])
        .assert()
        .code(0);
    let stops = kind_of(&lab, "x1", "stop");
    assert_eq!(stops.len(), 1);
    assert!(
        text(&a).contains(&format!("stop record: {}", stops[0].seq)),
        "{}",
        text(&a)
    );
    let s = status(&lab, "x1");
    assert_eq!(s["state"], "stopped");
    assert_eq!(s["detail"], "enough");
    let a = bin(&lab).args(["run", "x1"]).assert().code(1);
    assert!(err(&a).contains("stopped"), "{}", err(&a));
    let a = bin(&lab).args(["resume", "x1"]).assert().code(1);
    assert!(err(&a).contains("stopped"), "{}", err(&a));
    // A second stop adds nothing.
    bin(&lab)
        .args(["stop", "x1", "--reason", "again"])
        .assert()
        .code(0);
    assert_eq!(kind_of(&lab, "x1", "stop").len(), 1);
}

#[test]
fn stop_works_on_a_paused_study() {
    let lab = lab();
    plain(&lab, "x2");
    bin(&lab).args(["pause", "x2"]).assert().code(0);
    bin(&lab)
        .args(["stop", "x2", "--reason", "abandon"])
        .assert()
        .code(0);
    assert_eq!(status(&lab, "x2")["state"], "stopped");
}

#[test]
fn stop_needs_a_reason() {
    let lab = lab();
    plain(&lab, "x3");
    bin(&lab).args(["stop", "x3"]).assert().code(2);
}

#[test]
fn resume_and_stop_during_a_live_run() {
    let lab = lab();
    with_payload(&lab, "live", "#!/bin/sh\nsleep 20\n");
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("ignorarium"))
        .args(["run", "live"])
        .env("IGNORARIUM_HOME", &lab.home.root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let row_file = lab.home.study_dir("live").join("run/row.json");
    for _ in 0..200 {
        if row_file.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(row_file.exists(), "the row never started");
    assert_eq!(status(&lab, "live")["state"], "running");
    // A resume while the run is live clears the pause and starts nothing.
    bin(&lab).args(["pause", "live"]).assert().code(0);
    assert_eq!(status(&lab, "live")["request"], "pause");
    let a = bin(&lab).args(["resume", "live"]).assert().code(0);
    assert_eq!(text(&a), "the live run continues\n");
    assert_eq!(status(&lab, "live")["request"], "resume");
    let start = std::time::Instant::now();
    let a = bin(&lab)
        .args(["stop", "live", "--reason", "too slow"])
        .assert()
        .code(0);
    let stops = kind_of(&lab, "live", "stop");
    assert_eq!(stops.len(), 1);
    assert_eq!(text(&a), format!("stop record: {}\n", stops[0].seq));
    assert!(
        start.elapsed() < Duration::from_secs(15),
        "the run answered the stop"
    );
    assert_eq!(child.wait().unwrap().code().unwrap(), 1);
    assert_eq!(status(&lab, "live")["state"], "stopped");
}

/// Row 0 leaves a child that writes `late` after 1.5 s, marks itself started, and sleeps.
const SLEEPY_ROW_0: &str = r#"#!/bin/sh
if [ "$IGNORARIUM_ROW" = 0 ]; then
  (sleep 1.5; echo late > "$IGNORARIUM_OUT/late") &
  echo started > "$IGNORARIUM_OUT/started"
  sleep 30
fi
"#;

/// `ignorarium run <id>` in the background, through `env` with `env_args`, which `env`
/// applies before it runs the box in its own place (same process id).
fn start_run(lab: &Lab, id: &str, env_args: &[&str]) -> std::process::Child {
    std::process::Command::new("env")
        .args(env_args)
        .arg(assert_cmd::cargo::cargo_bin("ignorarium"))
        .args(["run", id])
        .env("IGNORARIUM_HOME", &lab.home.root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap()
}

/// The box's row file, once row 0 of the study at `dir` runs.
fn row_0_file(dir: &std::path::Path) -> Value {
    let t = std::time::Instant::now();
    loop {
        let row = std::fs::read(dir.join("run/row.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .filter(|_| dir.join("run/rows/0/out/started").exists());
        if let Some(row) = row {
            return row;
        }
        assert!(t.elapsed() < Duration::from_secs(20), "row 0 never started");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The box's exit status and stderr, or a panic when it runs on past 10 s.
fn ended(mut child: std::process::Child) -> (std::process::ExitStatus, String) {
    let t = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let mut stderr = String::new();
            std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut stderr).unwrap();
            return (status, stderr);
        }
        if t.elapsed() > Duration::from_secs(10) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("the box did not end");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Live processes of group `pgid`. A killed orphan may linger as a zombie until its new
/// parent reaps it; a zombie runs nothing.
fn live_in_group(pgid: u64) -> Vec<u64> {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|e| {
            let pid: u64 = e.ok()?.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
            let group: u64 = fields.get(2)?.parse().ok()?;
            (group == pgid && *fields.first()? != "Z").then_some(pid)
        })
        .collect()
}

fn send(sig: &str, pid: u32) {
    let sent = std::process::Command::new("kill")
        .args([format!("-{sig}"), pid.to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
}

/// Send `sig` `times` times to a box whose row 0 runs: the row's group dies, its spend is
/// recorded once, and the run pauses with exit 1.
fn a_signal_pauses_the_run(sig: &str, times: usize) {
    let lab = lab();
    let id = "sig";
    let dir = with_payload(&lab, id, SLEEPY_ROW_0);
    let child = start_run(&lab, id, &[]);
    let row = row_0_file(&dir);
    for _ in 0..times {
        send(sig, child.id());
    }
    let (code, stderr) = ended(child);
    let why = "interrupted by a signal";
    assert_eq!(code.code(), Some(1), "{code:?} {stderr}");
    assert_eq!(stderr, format!("error: paused: {why}\n"));
    // The row's whole group is gone: its child never writes.
    std::thread::sleep(Duration::from_millis(2000));
    assert!(dir.join("run/rows/0/out/started").is_file());
    assert!(!dir.join("run/rows/0/out/late").exists());
    assert_eq!(
        live_in_group(row["pgid"].as_u64().unwrap()),
        Vec::<u64>::new()
    );
    assert!(!dir.join("run/row.json").exists());
    let spend = kind_of(&lab, id, "spend");
    assert_eq!(spend.len(), 1);
    assert_eq!(spend[0].row, Some(0));
    assert_eq!(spend[0].body["reason"], why);
    assert!(spend[0].body["wall_seconds"].as_f64().unwrap() > 0.0);
    assert!(kind_of(&lab, id, "row").is_empty());
    assert_eq!(
        State::read(&lab.home, id).unwrap(),
        Some(State::Paused(why.into()))
    );
    let s = status(&lab, id);
    assert_eq!(s["state"], "paused");
    assert_eq!(s["detail"], why);
}

#[test]
fn sigint_kills_the_row_records_its_spend_and_pauses() {
    a_signal_pauses_the_run("INT", 1);
}

#[test]
fn sigterm_kills_the_row_records_its_spend_and_pauses() {
    a_signal_pauses_the_run("TERM", 1);
}

#[test]
fn sighup_kills_the_row_records_its_spend_and_pauses() {
    a_signal_pauses_the_run("HUP", 1);
}

#[test]
fn stop_sends_its_request_before_it_reads_a_torn_log() {
    let lab = lab();
    let id = "torn";
    let dir = with_payload(&lab, id, SLEEPY_ROW_0);
    let child = start_run(&lab, id, &[]);
    let row = row_0_file(&dir);
    // A torn last line, as a crash in mid-write leaves it.
    let log = dir.join("records.jsonl");
    let bytes = std::fs::read(&log).unwrap();
    std::fs::write(&log, &bytes[..bytes.len() - 10]).unwrap();
    let a = bin(&lab)
        .args(["stop", id, "--reason", "torn log"])
        .assert()
        .code(2);
    assert_eq!(
        err(&a),
        "error: the stop request is sent, but records of torn: line 1 is not a whole record\n"
    );
    assert_eq!(
        ControlFile::of(&lab.home, id).peek().unwrap(),
        Some(Signal::Stop("torn log".into()))
    );
    // The live run still sees the stop: its row's group dies and the box ends. The box
    // cannot add its stop record to the torn log, so it ends with an error.
    let (code, stderr) = ended(child);
    assert_eq!(code.code(), Some(2), "{stderr}");
    std::thread::sleep(Duration::from_millis(2000));
    assert!(!dir.join("run/rows/0/out/late").exists());
    assert_eq!(
        live_in_group(row["pgid"].as_u64().unwrap()),
        Vec::<u64>::new()
    );
}

#[test]
fn two_sighups_in_a_row_still_end_in_one_pause() {
    // A closing terminal sends SIGHUP twice: from the shell, then from the kernel.
    a_signal_pauses_the_run("HUP", 2);
}

/// Row 0 marks itself started, then takes a second.
const SLOW_ROW_0: &str = r#"#!/bin/sh
if [ "$IGNORARIUM_ROW" = 0 ]; then
  echo started > "$IGNORARIUM_OUT/started"
  sleep 1
fi
"#;

#[test]
fn a_box_started_with_sighup_ignored_runs_on_through_a_sighup() {
    let lab = lab();
    let id = "nohup";
    let dir = with_payload(&lab, id, SLOW_ROW_0);
    // As `nohup` starts it.
    let child = start_run(&lab, id, &["--ignore-signal=HUP"]);
    row_0_file(&dir);
    send("HUP", child.id());
    let (code, stderr) = ended(child);
    assert_eq!(code.code(), Some(0), "{code:?} {stderr}");
    assert_eq!(kind_of(&lab, id, "row").len(), 6);
    assert!(kind_of(&lab, id, "spend").is_empty());
    assert_eq!(status(&lab, id)["state"], "done");
}

#[test]
fn a_box_started_with_sigchld_ignored_still_sees_each_row_end() {
    let lab = lab();
    plain(&lab, "chld");
    // An ignored SIGCHLD lets the kernel reap children before the box can look at them.
    let child = start_run(&lab, "chld", &["--ignore-signal=CHLD"]);
    let (code, stderr) = ended(child);
    assert_eq!(code.code(), Some(0), "{stderr}");
    let rows = kind_of(&lab, "chld", "row");
    assert_eq!(rows.len(), 6);
    for r in &rows {
        assert_eq!(r.body["exit"], "ok");
        assert_eq!(r.body["attempts"], 1);
    }
}

#[test]
fn pause_refuses_a_stopped_study() {
    let lab = lab();
    plain(&lab, "ps");
    bin(&lab)
        .args(["stop", "ps", "--reason", "x"])
        .assert()
        .code(0);
    let a = bin(&lab).args(["pause", "ps"]).assert().code(1);
    assert!(err(&a).contains("stopped"), "{}", err(&a));
    assert_eq!(
        ControlFile::of(&lab.home, "ps").peek().unwrap(),
        Some(Signal::Stop("x".into()))
    );
}

#[test]
fn run_refuses_a_bad_machine_file_with_exit_2() {
    let lab = lab();
    plain(&lab, "bm");
    std::fs::write(lab.home.root.join("machine.toml"), "cpus = 0\n").unwrap();
    let a = bin(&lab).args(["run", "bm"]).assert().code(2);
    assert!(err(&a).contains("machine.toml"), "{}", err(&a));
}

/// Signed records with known spend on a study that never ran.
fn plant(lab: &Lab, id: &str, rows: &[(u64, &str, &str, f64, f64, u64)]) {
    let key = keys::load(&lab.home).unwrap().unwrap();
    let log = lab.home.study_dir(id).join("records.jsonl");
    for &(row, condition, exit, cpu, wall, attempts) in rows {
        let draft = ignorarium::records::RecordDraft {
            study: id.into(),
            row: Some(row),
            kind: "row".into(),
            body: serde_json::json!({
                "condition": condition, "exit": exit, "cpu_seconds": cpu,
                "wall_seconds": wall, "attempts": attempts,
            }),
        };
        ignorarium::records::append(&log, &key, draft).unwrap();
    }
    plant_spend(lab, id, 1800.0, 0.0);
}

/// A signed spend record, as a stopped row leaves.
fn plant_spend(lab: &Lab, id: &str, cpu: f64, wall: f64) {
    let key = keys::load(&lab.home).unwrap().unwrap();
    let log = lab.home.study_dir(id).join("records.jsonl");
    let spend = ignorarium::records::RecordDraft {
        study: id.into(),
        row: Some(99),
        kind: "spend".into(),
        body: serde_json::json!({ "cpu_seconds": cpu, "wall_seconds": wall, "reason": "x" }),
    };
    ignorarium::records::append(&log, &key, spend).unwrap();
}

#[test]
fn status_values_are_exact_for_planted_records() {
    let lab = lab();
    std::fs::write(
        lab.home.root.join("machine.toml"),
        "max_level = 1\nrate_usd_per_hour = 2.0\n",
    )
    .unwrap();
    plain(&lab, "ex");
    plant(
        &lab,
        "ex",
        &[
            (0, "exact", "ok", 1800.0, 3600.0, 2),
            (1, "arm-a", "payload:3", 0.0, 0.0, 1),
        ],
    );
    let s = status(&lab, "ex");
    let total = s["total"].as_u64().unwrap();
    assert_eq!(s["done"], 2);
    assert_eq!(s["voided"], 1);
    assert_eq!(s["retried"], 1);
    assert_eq!(s["cost"]["cpu_hours"], 1.0);
    assert_eq!(s["cost"]["wall_hours"], 1.0);
    assert_eq!(s["cost"]["money_usd"], 2.0);
    assert_eq!(s["used"]["cpu_hours"], 1.0);
    assert_eq!(s["used"]["wall_hours"], 1.0);
    assert!(s["used"]["money_usd"].is_null(), "no cap, no fraction");
    assert_eq!(s["elapsed_seconds"], 3600.0);
    assert_eq!(s["rows_per_hour"], 2.0);
    assert_eq!(s["cost_rate_usd_per_hour"], 2.0);
    assert_eq!(
        s["eta_seconds"].as_f64().unwrap(),
        (total - 2) as f64 / 2.0 * 3600.0
    );
    assert_eq!(s["conditions"][0]["done"], 1);
    assert_eq!(s["conditions"][1]["done"], 1);
    assert_eq!(s["conditions"][0]["total"], 3);
    assert_eq!(s["conditions"][1]["total"], 3);
    let a = bin(&lab).args(["status", "ex"]).assert().success();
    let t = text(&a);
    for want in [
        "rows: 2/".to_string(),
        "rate: 2.00 rows/hour".to_string(),
        "elapsed: 3600.00 s".to_string(),
        format!("eta: {:.2} s", (total - 2) as f64 / 2.0 * 3600.0),
        "cost: cpu 1.000 h, wall 1.000 h, money $2.000".to_string(),
        "(100%)".to_string(),
        "(no cap)".to_string(),
        "cost rate: 2.00 USD/hour".to_string(),
        "voided: 1\nretried: 1\nblind: no\n".to_string(),
    ] {
        assert!(t.contains(&want), "{want} in {t}");
    }
}

#[test]
fn status_rates_and_shares_divide_by_hours_and_caps() {
    let lab = lab();
    let caps = |t: String| {
        t.replace("cpu_hours = 1.0", "cpu_hours = 2.0")
            .replace("wall_hours = 1.0", "wall_hours = 4.0")
    };
    register_study(&lab, "half", caps, &[], common::PAYLOAD);
    // Two rows in half an hour of wall; with the spend record, one cpu hour.
    plant(
        &lab,
        "half",
        &[
            (0, "exact", "ok", 900.0, 900.0, 1),
            (1, "arm-a", "ok", 900.0, 900.0, 1),
        ],
    );
    let s = status(&lab, "half");
    assert_eq!(s["rows_per_hour"], 4.0);
    assert_eq!(s["used"]["cpu_hours"], 0.5);
    assert_eq!(s["used"]["wall_hours"], 0.125);
}

#[test]
fn status_gives_no_rate_without_both_rows_and_time() {
    let lab = lab();
    // Spend but no row: no rate.
    plain(&lab, "spent");
    plant_spend(&lab, "spent", 0.0, 600.0);
    let s = status(&lab, "spent");
    assert_eq!(s["done"], 0);
    assert!(s["rows_per_hour"].is_null(), "{s}");
    // A row but no wall time: no rate, so no time left either.
    plain(&lab, "instant");
    plant(&lab, "instant", &[(0, "exact", "ok", 0.0, 0.0, 1)]);
    let s = status(&lab, "instant");
    assert_eq!(s["done"], 1);
    assert!(s["rows_per_hour"].is_null(), "{s}");
    assert!(s["eta_seconds"].is_null(), "{s}");
}

#[test]
fn blind_labels_follow_the_seeded_order() {
    let lab = lab();
    let dir = register_study(
        &lab,
        "bl",
        |t| t.replace("blind = false", "blind = true"),
        &[],
        common::PAYLOAD,
    );
    plant(&lab, "bl", &[(0, "exact", "ok", 1.0, 1.0, 1)]);
    let hash =
        ignorarium::spec::hash_bytes(&ignorarium::spec::load(&dir.join("spec.toml")).unwrap().hash)
            .unwrap();
    let order = blind_order(&hash, &["exact".to_string(), "arm-a".to_string()]);
    let at = order.iter().position(|c| c == "exact").unwrap();
    let s = status(&lab, "bl");
    let conds = s["conditions"].as_array().unwrap();
    assert_eq!(conds[at]["done"], 1);
    assert_eq!(conds[1 - at]["done"], 0);
    assert_eq!(
        conds[at]["label"],
        if at == 0 {
            "condition A"
        } else {
            "condition B"
        }
    );
    let a = bin(&lab).args(["status", "bl"]).assert().success();
    assert!(text(&a).contains("blind: yes\n"));
}

#[test]
fn status_order_stop_then_live_then_interrupted_then_state() {
    let lab = lab();
    plain(&lab, "o1");
    // Never run.
    let s = status(&lab, "o1");
    assert_eq!(s["state"], "not started");
    assert!(
        !lab.home.study_dir("o1").join("run").exists(),
        "reads make no run/"
    );
    // State Running with a free lock: the box died.
    State::Running.write(&lab.home, "o1").unwrap();
    assert_eq!(status(&lab, "o1")["state"], "interrupted");
    // Lock held: live, with a pending request shown.
    let held = RunLock::try_take(&lab.home, "o1").unwrap().unwrap();
    ControlFile::of(&lab.home, "o1").request_pause().unwrap();
    let s = status(&lab, "o1");
    assert_eq!(s["state"], "running");
    assert_eq!(s["request"], "pause");
    drop(held);
    // A stop record outranks everything, a held lock included.
    let key = keys::load(&lab.home).unwrap().unwrap();
    let held = RunLock::try_take(&lab.home, "o1").unwrap().unwrap();
    held.stop(&key, "done here", None).unwrap();
    State::Running.write(&lab.home, "o1").unwrap();
    assert_eq!(status(&lab, "o1")["state"], "stopped");
    drop(held);
    // The paused state reads from state.json.
    plain(&lab, "o2");
    State::Paused("the money cap is reached".into())
        .write(&lab.home, "o2")
        .unwrap();
    let s = status(&lab, "o2");
    assert_eq!(s["state"], "paused");
    assert_eq!(s["detail"], "the money cap is reached");
    let a = bin(&lab).args(["status", "o1"]).assert().success();
    assert!(text(&a).contains("state: stopped"), "{}", text(&a));
}

#[test]
fn status_json_has_every_field() {
    let lab = lab();
    plain(&lab, "j1");
    bin(&lab).args(["run", "j1"]).assert().code(0);
    let s = status(&lab, "j1");
    let obj = s.as_object().unwrap();
    for key in [
        "study",
        "state",
        "detail",
        "request",
        "done",
        "total",
        "rows_per_hour",
        "elapsed_seconds",
        "eta_seconds",
        "cost",
        "budget",
        "used",
        "cost_rate_usd_per_hour",
        "voided",
        "retried",
        "triggers",
        "blind",
        "conditions",
    ] {
        assert!(obj.contains_key(key), "missing {key}: {s}");
    }
    for dim in ["cpu_hours", "wall_hours", "money_usd"] {
        assert!(s["cost"][dim].is_number(), "{dim}");
        assert!(s["budget"][dim].is_number(), "{dim}");
    }
    assert_eq!(s["study"], "j1");
    assert_eq!(s["budget"]["cpu_hours"], 1.0);
    assert_eq!(s["voided"], 0);
    assert_eq!(s["retried"], 0);
    assert_eq!(s["triggers"], serde_json::json!([]));
    assert_eq!(s["blind"], false);
    let conds = s["conditions"].as_array().unwrap();
    let ids: Vec<&str> = conds.iter().map(|c| c["label"].as_str().unwrap()).collect();
    assert_eq!(ids, ["exact", "arm-a"]);
    let sum: u64 = conds.iter().map(|c| c["done"].as_u64().unwrap()).sum();
    assert_eq!(sum, s["done"]);
    assert_eq!(s["eta_seconds"], 0.0);
    // Text carries the same facts.
    let a = bin(&lab).args(["status", "j1"]).assert().success();
    let t = text(&a);
    for want in [
        "state: done",
        "rows:",
        "elapsed:",
        "eta:",
        "cost:",
        "budget:",
        "voided: 0",
        "retried: 0",
        "triggers: none",
        "arm-a:",
    ] {
        assert!(t.contains(want), "{want} in {t}");
    }
}

#[test]
fn status_counts_void_rows_and_retries() {
    let lab = lab();
    with_payload(&lab, "v1", "#!/bin/sh\nexit 3\n");
    bin(&lab).args(["run", "v1"]).assert().code(0);
    let s = status(&lab, "v1");
    assert_eq!(s["voided"], s["total"]);
    assert_eq!(s["done"], s["total"]);
    assert_eq!(s["retried"], 0);
}

#[test]
fn status_before_any_row_has_null_rates() {
    let lab = lab();
    plain(&lab, "n1");
    let s = status(&lab, "n1");
    assert_eq!(s["done"], 0);
    assert!(s["total"].as_u64().unwrap() > 0);
    assert!(s["rows_per_hour"].is_null());
    assert!(s["eta_seconds"].is_null());
    assert_eq!(s["elapsed_seconds"], 0.0);
}

#[test]
fn status_blinds_condition_ids_when_blind_is_set() {
    let lab = lab();
    register_study(
        &lab,
        "b1",
        |t| t.replace("blind = false", "blind = true"),
        &[("data/example.txt", "calibration")],
        common::PAYLOAD,
    );
    bin(&lab).args(["run", "b1"]).assert().code(0);
    let a = bin(&lab).args(["status", "b1"]).assert().success();
    let t = text(&a);
    assert!(!t.contains("arm-a") && !t.contains("exact"), "{t}");
    let a = bin(&lab)
        .args(["status", "b1", "--json"])
        .assert()
        .success();
    let raw = text(&a);
    assert!(!raw.contains("arm-a") && !raw.contains("exact"), "{raw}");
    let s: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(s["blind"], true);
    let labels: Vec<&str> = s["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["condition A", "condition B"]);
    // The label order is fixed by the spec hash: the same on a second read.
    assert_eq!(status(&lab, "b1")["conditions"], s["conditions"]);
}

#[test]
fn the_blind_order_is_seeded_by_the_spec_hash() {
    let ids = ["exact".to_string(), "arm-a".to_string()];
    let order = |n: u8| blind_order(&[n; 32], &ids);
    assert_eq!(order(1), order(1));
    let orders: std::collections::BTreeSet<_> = (0..16).map(order).collect();
    assert_eq!(orders.len(), 2, "both orders must occur");
    for o in &orders {
        let mut sorted = o.clone();
        sorted.sort();
        assert_eq!(sorted, ["arm-a", "exact"], "a permutation, nothing dropped");
    }
}

#[test]
fn guide_lists_run_and_status_with_real_commands() {
    let lab = lab();
    let a = bin(&lab).args(["guide", "--json"]).assert().success();
    let steps: Value = serde_json::from_str(&text(&a)).unwrap();
    let find = |id: &str| {
        steps
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == id)
            .unwrap_or_else(|| panic!("{id}"))
            .clone()
    };
    let run = find("run");
    assert_eq!(run["command"], "ignorarium run <id>");
    assert!(!run.to_string().contains("not available yet"));
    // Level 0 isolates nothing, so the step promises no sandbox.
    assert!(!run.to_string().contains("sandbox"), "{run}");
    assert!(
        run["does"]
            .as_str()
            .unwrap()
            .contains("no network or file isolation")
    );
    let status = find("status");
    assert_eq!(status["command"], "ignorarium status <id>");
    assert!(!status.to_string().contains("not available yet"));
    let ids: Vec<&str> = steps
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    let at = |id: &str| ids.iter().position(|i| *i == id).unwrap();
    assert!(at("register") < at("run") && at("run") < at("status") && at("status") < at("report"));
    assert!(!text(&a).contains("Plan "));
}

#[test]
fn run_refuses_while_another_run_holds_the_lock() {
    let lab = lab();
    plain(&lab, "bz");
    let _held = RunLock::try_take(&lab.home, "bz").unwrap().unwrap();
    let a = bin(&lab).args(["run", "bz"]).assert().code(1);
    assert!(err(&a).contains("in progress"), "{}", err(&a));
}
