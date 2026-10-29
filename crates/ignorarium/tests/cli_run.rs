//! The run verbs, driven through the real binary at isolation level 0.
mod common;
use assert_cmd::Command;
use common::{Lab, kind_of, lab, plain, register_study, with_payload};
use ignorarium::cli::status::blind_order;
use ignorarium::cli::stop::{Stopped, stop_study};
use ignorarium::control::{ControlFile, RunLock, State};
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
    let a = bin(&lab).arg("probe").assert().code(0);
    let out = text(&a);
    assert!(out.contains("max level: 0"), "{out}");
    assert!(out.contains("/dev/kvm"), "{out}");
    let m = ignorarium::machine::load(&lab.home.root).unwrap();
    assert_eq!(m.max_level, 0);
    assert_eq!(m.rate_usd_per_hour, 2.5);
    let events = common::events_of(&lab.home.root);
    let probe = common::named(&events, "probe");
    assert_eq!(probe.len(), 1);
    assert_eq!(probe[0]["attrs"]["max_level"], 0);
    assert!(probe[0]["attrs"]["kvm"].is_boolean());
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
fn max_level_flag_caps_the_backends() {
    let lab = lab();
    std::fs::write(lab.home.root.join("machine.toml"), "max_level = 2\n").unwrap();
    register_study(
        &lab,
        "cap",
        |t| t.replace("min_isolation = 0", "min_isolation = 1"),
        &[],
        common::PAYLOAD,
    );
    // Only level 0 exists in this build, so level 1 is refused whatever the flag says.
    let a = bin(&lab)
        .args(["run", "cap", "--max-level", "0"])
        .assert()
        .code(1);
    assert!(err(&a).contains("ignorarium probe"), "{}", err(&a));
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
fn stop_during_a_live_run_is_recorded_by_the_run() {
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
    let s = status(&lab, "live");
    assert_eq!(s["state"], "running");
    bin(&lab)
        .args(["stop", "live", "--reason", "too slow"])
        .assert()
        .code(0);
    let code = child.wait().unwrap().code().unwrap();
    assert_eq!(code, 1);
    assert_eq!(kind_of(&lab, "live", "stop").len(), 1);
    assert_eq!(status(&lab, "live")["state"], "stopped");
}

#[test]
fn stop_while_a_live_run_never_answers_says_the_run_records_it() {
    let lab = lab();
    plain(&lab, "w1");
    let key = keys::load(&lab.home).unwrap().unwrap();
    let held = RunLock::try_take(&lab.home, "w1").unwrap().unwrap();
    let got = stop_study(&lab.home, &key, "w1", "later", Duration::from_millis(200)).unwrap();
    assert!(matches!(got, Stopped::Requested));
    drop(held);
    assert_eq!(
        ControlFile::of(&lab.home, "w1").peek().unwrap(),
        Some(ignorarium::control::Signal::Stop("later".into()))
    );
    let again = stop_study(&lab.home, &key, "w1", "later", Duration::from_millis(200)).unwrap();
    assert!(matches!(again, Stopped::Recorded(_)));
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
