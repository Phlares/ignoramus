//! The run verbs, driven through the real binary. The test machine file reaches level 1,
//! so rows run in bubblewrap.
mod common;
use assert_cmd::Command;
use common::{Lab, kind_of, lab, no_level1, plain, register_study, with_payload};
use ignorarium::backend::level2::Level2;
use ignorarium::backend::{Backend, Probe};
use ignorarium::canon::sha256_hex;
use ignorarium::cli::status::blind_order;
use ignorarium::control::{ControlFile, RunLock, Signal, State};
use ignorarium::derived::Register;
use ignorarium::estimate::apply_defaults;
use ignorarium::spec::model::Spec;
use ignorarium::{keys, view};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

fn bin(lab: &Lab) -> Command {
    let mut c = Command::cargo_bin("ignorarium").unwrap();
    c.env("IGNORARIUM_HOME", &lab.home.root);
    c
}

/// `bin` in the root of study `id`, where `run` and `resume` read a late input from.
fn bin_at(lab: &Lab, id: &str) -> Command {
    let mut c = bin(lab);
    c.current_dir(common::work(lab, id));
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
fn a_register_record_that_does_not_parse_is_not_a_registered_study() {
    let lab = lab();
    let dir = plain(&lab, "noroot");
    let key = keys::load(&lab.home).unwrap().unwrap();
    let log = dir.join("records.jsonl");
    std::fs::rename(&log, dir.join("records.old")).unwrap();
    let draft = ignorarium::records::RecordDraft {
        study: "noroot".into(),
        row: None,
        kind: "register".into(),
        body: serde_json::json!({"spec_hash": "sha256:ab", "leaf": 1, "log": "planlog", "defaults": []}),
    };
    ignorarium::records::append(&log, &key, draft).unwrap();
    let a = bin(&lab).args(["status", "noroot"]).assert().code(1);
    assert!(err(&a).contains("is not registered"), "{}", err(&a));
}

#[test]
fn probe_writes_machine_toml_and_prints_the_level() {
    let lab = lab();
    std::fs::write(
        lab.home.root.join("machine.toml"),
        "max_level = 0\nrate_usd_per_hour = 2.5\n",
    )
    .unwrap();
    // The highest level that probes available here.
    let level2 = Level2::new().probe();
    let want = match (&level2, no_level1()) {
        (Probe::Available, _) => 2,
        (_, None) => 1,
        _ => 0,
    };
    let a = bin(&lab).arg("probe").assert().code(0);
    let out = text(&a);
    assert!(out.contains(&format!("max level: {want}")), "{out}");
    assert!(out.contains("level 2 ("), "{out}");
    // An unavailable level 2 says why, its first fault on its own line.
    if let Probe::Unavailable(why) = &level2 {
        let first = why.split("; ").next().unwrap();
        let line = out.lines().find(|l| l.starts_with("level 2 (")).unwrap();
        assert!(line.contains(&format!("): unavailable: {first}")), "{out}");
    }
    let kvm = if Path::new("/dev/kvm").exists() {
        "present"
    } else {
        "absent"
    };
    assert!(out.ends_with(&format!("/dev/kvm: {kvm}\n")), "{out}");
    assert!(out.contains("level 0 (process): available\n"), "{out}");
    if want >= 1 {
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
    let a = bin_at(&lab, "s1").args(["run", "s1"]).assert().code(0);
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
fn editing_the_working_payload_after_register_does_not_reach_the_run() {
    let lab = lab();
    let dir = plain(&lab, "frozen");
    let registered = kind_of(&lab, "frozen", "register")[0].body["payload"].clone();
    // The author edits the working payload after registration; the box runs its copy.
    let work = common::work(&lab, "frozen").join("payload/run");
    common::write_exec(
        &work,
        "#!/bin/sh\necho edited > \"$IGNORARIUM_OUT/result\"\nprintf '{\"ppl\": 1.5}' > \"$IGNORARIUM_OUT/values.json\"\n",
    );
    bin_at(&lab, "frozen")
        .args(["run", "frozen"])
        .assert()
        .code(0);
    let rows = kind_of(&lab, "frozen", "row");
    assert_eq!(rows.len(), 6);
    for r in &rows {
        assert_eq!(r.body["exit"], "ok");
        assert_eq!(r.body["payload_sha256"], registered);
        let out = dir.join(format!("run/rows/{}/out/result", r.row.unwrap()));
        let got = std::fs::read_to_string(out).unwrap();
        let want = format!(
            "{} {} {}",
            r.body["condition"].as_str().unwrap(),
            r.body["unit"].as_str().unwrap(),
            r.body["repeat"]
        );
        assert_eq!(got, want);
    }
}

#[test]
fn rows_see_exactly_the_declared_inputs() {
    let lab = lab();
    // A model the store holds and the root does not, pinned by digest.
    let model = lab.root.join("weights.bin");
    std::fs::write(&model, [0u8, 1, 2, 3]).unwrap();
    let store = ignorarium::store::Store::open(&lab.home.root).unwrap();
    let (digest, _) = store.import(&model).unwrap();
    let declared = move |t: String| {
        format!(
            "{t}\n[[inputs]]\npath = \"models/m.bin\"\nsha256 = \"{digest}\"\nrole = \"model\"\n\
             [inputs.licence]\nspdx = \"\"\ntype = \"unspecified\"\nsource = \"\"\ncreators = []\n"
        )
    };
    let payload = "#!/bin/sh\ncd \"$IGNORARIUM_IN\" && find . | LC_ALL=C sort > \"$IGNORARIUM_OUT/seen\"\nprintf '{\"ppl\": 1.5}' > \"$IGNORARIUM_OUT/values.json\"\n";
    // The root also holds a file no input names.
    let before = [
        ("data/example.txt", "calibration"),
        ("data/other.txt", "not declared"),
    ];
    let dir = register_study(&lab, "view", declared, &before, payload);
    bin_at(&lab, "view").args(["run", "view"]).assert().code(0);
    let rows = kind_of(&lab, "view", "row");
    assert_eq!(rows.len(), 6);
    for r in &rows {
        assert_eq!(r.body["exit"], "ok", "{:?}", r.body);
        let seen = dir.join(format!("run/rows/{}/out/seen", r.row.unwrap()));
        assert_eq!(
            std::fs::read_to_string(seen).unwrap(),
            ".\n./data\n./data/example.txt\n./ignorarium:inputs.json\n./models\n./models/m.bin\n"
        );
    }
}

#[test]
fn rows_read_the_manifest() {
    let lab = lab();
    let payload = format!(
        "#!/bin/sh\ncp \"$IGNORARIUM_IN/{}\" \"$IGNORARIUM_OUT/m\"\n{}\n",
        view::MANIFEST,
        common::VALUES
    );
    let dir = with_payload(&lab, "list", &payload);
    bin_at(&lab, "list").args(["run", "list"]).assert().code(0);
    let mut spec = ignorarium::spec::load(&dir.join("spec.toml"))
        .unwrap()
        .value;
    apply_defaults(&mut spec);
    let spec = Spec::from_value(&spec).unwrap();
    let register = Register::of(&kind_of(&lab, "list", "register")[0]).unwrap();
    let want = view::manifest(&spec, &register.inputs);
    // The template's confirmation input is absent, so only the calibration input is listed.
    let listed: Value = serde_json::from_slice(&want).unwrap();
    assert_eq!(
        listed,
        serde_json::json!([{
            "path": "data/example.txt",
            "role": "calibration",
            "sha256": sha256_hex(b"calibration"),
        }])
    );
    let rows = kind_of(&lab, "list", "row");
    assert_eq!(rows.len(), 6);
    for r in &rows {
        assert_eq!(r.body["exit"], "ok", "{:?}", r.body);
        let got = std::fs::read(dir.join(format!("run/rows/{}/out/m", r.row.unwrap()))).unwrap();
        assert_eq!(got, want);
        assert_eq!(r.body["manifest"], sha256_hex(&want));
    }
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
    let a = bin_at(&lab, "hi").args(["run", "hi"]).assert().code(1);
    let e = err(&a);
    assert!(e.contains("level 2"), "{e}");
    assert!(e.contains("ignorarium probe"), "{e}");
    assert!(kind_of(&lab, "hi", "row").is_empty());
    // The file allows level 2, but it cannot run here now: the level named is one that can.
    std::fs::write(lab.home.root.join("machine.toml"), "max_level = 2\n").unwrap();
    let mut run = bin_at(&lab, "hi");
    for var in [
        "IGNORARIUM_VM_VMM",
        "IGNORARIUM_VM_KERNEL",
        "IGNORARIUM_VM_AGENT",
    ] {
        run.env_remove(var);
    }
    let e = err(&run.args(["run", "hi"]).assert().code(1));
    assert!(
        e.contains("needs isolation level 2; this machine reaches level "),
        "{e}"
    );
    assert!(!e.contains("reaches level 2"), "{e}");
    assert!(kind_of(&lab, "hi", "row").is_empty());
}

#[test]
fn max_level_flag_parses_and_a_value_above_3_is_a_usage_error() {
    let lab = lab();
    plain(&lab, "ml");
    bin_at(&lab, "ml")
        .args(["run", "ml", "--max-level", "4"])
        .assert()
        .code(2);
    assert!(kind_of(&lab, "ml", "row").is_empty());
    bin_at(&lab, "ml")
        .args(["run", "ml", "--max-level", "0"])
        .assert()
        .code(0);
    assert_eq!(status(&lab, "ml")["state"], "done");
}

#[test]
fn run_outside_the_study_root_is_refused_when_an_input_was_absent_at_registration() {
    let lab = lab();
    plain(&lab, "out");
    let a = bin(&lab)
        .current_dir(&lab.root)
        .args(["run", "out"])
        .assert()
        .code(1);
    assert!(
        err(&a).contains("input data/confirm.txt was absent at registration"),
        "{}",
        err(&a)
    );
    assert!(kind_of(&lab, "out", "row").is_empty());
    bin_at(&lab, "out").args(["run", "out"]).assert().code(0);
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
    let a = bin_at(&lab, "p1").args(["run", "p1"]).assert().code(1);
    assert!(
        err(&a).contains("a pause is pending; run `ignorarium resume p1`"),
        "{}",
        err(&a)
    );
    assert!(kind_of(&lab, "p1", "row").is_empty());
    // resume clears the pause and runs to the end.
    bin_at(&lab, "p1").args(["resume", "p1"]).assert().code(0);
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
    let a = bin_at(&lab, "bud").args(["run", "bud"]).assert().code(1);
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
    let a = bin_at(&lab, "x1").args(["run", "x1"]).assert().code(1);
    assert!(err(&a).contains("stopped"), "{}", err(&a));
    let a = bin_at(&lab, "x1").args(["resume", "x1"]).assert().code(1);
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
        .current_dir(common::work(&lab, "live"))
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
    let a = bin_at(&lab, "live")
        .args(["resume", "live"])
        .assert()
        .code(0);
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
printf '{"ppl": 1.5}' > "$IGNORARIUM_OUT/values.json"
"#;

/// `ignorarium run <id>` in the background, through `env` with `env_args`, which `env`
/// applies before it runs the box in its own place (same process id).
fn start_run(lab: &Lab, id: &str, env_args: &[&str]) -> std::process::Child {
    std::process::Command::new("env")
        .args(env_args)
        .arg(assert_cmd::cargo::cargo_bin("ignorarium"))
        .args(["run", id])
        .current_dir(common::work(lab, id))
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
        "error: the stop request is sent, but records of torn: line 2 is not a whole record\n"
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
printf '{"ppl": 1.5}' > "$IGNORARIUM_OUT/values.json"
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
    let a = bin_at(&lab, "bm").args(["run", "bm"]).assert().code(2);
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
    bin_at(&lab, "j1").args(["run", "j1"]).assert().code(0);
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
        "unknown_cpu",
        "triggers",
        "blind",
        "conditions",
        "resize",
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
    assert_eq!(s["unknown_cpu"], 0);
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
        "rows with unknown cpu: 0",
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
    bin_at(&lab, "v1").args(["run", "v1"]).assert().code(0);
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
    bin_at(&lab, "b1").args(["run", "b1"]).assert().code(0);
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
    let payload = find("payload");
    assert!(
        payload["does"].as_str().unwrap().contains(view::MANIFEST),
        "{payload}"
    );
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
    let a = bin_at(&lab, "bz").args(["run", "bz"]).assert().code(1);
    assert!(err(&a).contains("in progress"), "{}", err(&a));
}

// ---- a pilot's estimates serve a follow-on ----

/// The template with a second confirmation input: 2 clusters of 3 units.
fn two_clusters(t: String) -> String {
    let second = r#"[[inputs]]
path = "data/confirm2.txt"
sha256 = ""
role = "confirmation"
stratum = "fiction"
units = 3
[inputs.licence]
spdx = ""
type = "unspecified"
source = ""
creators = []

[pins]"#;
    t.replacen("[pins]", second, 1)
}

/// A payload that writes `base` for the reference and `base + step * k` for `arm-a` on
/// unit `k` of the first cluster and `base + step * (k + 4)` on the second: arm-a pairs to
/// 100 * step / base percent per k.
fn graded(base: f64, step: f64) -> String {
    let mut cases = String::new();
    for (path, offset) in [("data/confirm.txt", 0), ("data/confirm2.txt", 4)] {
        for k in 0..3 {
            let v = base + step * f64::from(k + offset + 1);
            cases += &format!("\"{path}#{k}\") v={v};; ");
        }
    }
    format!(
        "#!/bin/sh\ncase \"$2\" in exact) v={base};; *) case \"$4\" in {cases}esac;; esac\nprintf '{{\"ppl\": %s}}' \"$v\" > \"$IGNORARIUM_OUT/values.json\"\n"
    )
}

/// Register and run a pilot study `id` at level 0.
fn pilot_run(lab: &Lab, id: &str, payload: &str) {
    use ignorarium::backend::level0::Level0;
    use ignorarium::runner::{RunStatus, prepare_with, run};
    register_study(lab, id, two_clusters, &[], payload);
    let plan = prepare_with(
        &lab.home,
        id,
        &common::work_of(&lab.home, id),
        vec![Box::new(Level0)],
    )
    .unwrap();
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
}

fn check_json(lab: &Lab, spec: &std::path::Path) -> (i32, Value) {
    let a = bin(lab).arg("check").arg(spec).arg("--json").assert();
    let out = a.get_output();
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

/// The codes of the diagnostics that concern the plan's values, not its licences or pins.
fn codes(v: &Value) -> Vec<&str> {
    let d = v["diagnostics"].as_array().unwrap();
    let all = d.iter().map(|d| d["code"].as_str().unwrap());
    all.filter(|c| !c.starts_with("W-LIC") && !c.starts_with("W-HSH"))
        .collect()
}

#[test]
fn a_pilot_then_a_follow_on_resolve_end_to_end() {
    let lab = lab();
    // Cluster means 0.2 % and 0.6 % (percent values 0.1 k): var 0.0867, icc 0.885.
    pilot_run(&lab, "my-pilot", &graded(1000.0, 1.0));
    // The same shape, 1000 times wider.
    pilot_run(&lab, "wide-pilot", &graded(1.0, 1.0));
    let est = kind_of(&lab, "my-pilot", "estimate");
    assert_eq!(est.len(), 1);
    let e = ignorarium::derived::Estimate::of(&est[0]).unwrap();
    assert_eq!((e.clusters, e.units, e.stratum.as_str()), (2, 6, ""));
    assert!((e.var - 0.086_666_666_7).abs() < 1e-6, "{}", e.var);
    assert!(
        (e.icc.unwrap() - 2300.0 / 2600.0).abs() < 1e-6,
        "{:?}",
        e.icc
    );
    let follow = |from: &str, range: &str| {
        let t = std::fs::read_to_string("../../templates/numeric-ablation.toml").unwrap();
        let path = lab.root.join(format!("follow-{from}.toml"));
        // A claim on a mean statistic in `range`, sized 20 below the margin: in [-22, 22],
        // with a small variance each of the 16 clusters scores about 0.91, enough to reach
        // 1/0.005.
        let text = common::confirmatory(two_clusters(t))
            .replace("{ quantile = 0.5 }", &format!("{{ mean = {range} }}"));
        std::fs::write(&path, text.replace("my-pilot", from)).unwrap();
        path
    };
    // The estimate resolves: no pending value, no missing estimate, and the power gate
    // passes on the small variance.
    let (code, v) = check_json(&lab, &follow("my-pilot", "[-22.0, 22.0]"));
    assert_eq!((code, codes(&v)), (0, Vec::<&str>::new()), "{v}");
    assert_eq!(v["pending"], serde_json::json!([]));
    // The wide pilot's units, 100 % to 700 %, all lie past 22: clipped, each counts as 22,
    // and no spread is left to size from.
    let (code, v) = check_json(&lab, &follow("wide-pilot", "[-22.0, 22.0]"));
    assert_eq!((code, codes(&v)), (1, vec!["E-CNF-003"]), "{v}");
    assert!(
        (v["diagnostics"].as_array().unwrap().iter())
            .any(|d| d["problem"].as_str().unwrap().ends_with(
                "Study wide-pilot's unit values of this contrast in the pooled group, clipped to the statistic's range, give no icc, or no variance above 0. A range that every value lies past leaves no spread: widen it, or borrow from another study."
            )),
        "{v}"
    );
    // In a range that holds them it resolves, and the gate refuses on its variance alone.
    let (code, v) = check_json(&lab, &follow("wide-pilot", "[-1000.0, 1000.0]"));
    assert_eq!((code, codes(&v)), (1, vec!["E-CNF-002"]), "{v}");
}

// ---- a study that re-sizes once from its reserve ----

/// The reserve inputs of the re-size studies here.
const RESERVE: usize = 32;
const FIRST: usize = 15;

/// A payload that writes 100 for the reference and `arm` for `arm-a` on every unit.
fn flat(arm: &str) -> String {
    format!(
        "#!/bin/sh\ncase \"$2\" in exact) v=100;; *) v={arm};; esac\nprintf '{{\"ppl\": %s}}' \"$v\" > \"$IGNORARIUM_OUT/values.json\"\n"
    )
}

/// Register re-size study `id` (`common::resize_plan`) with `payload`, in a lab that holds
/// `common::resize_pilot`, and prepare its run at level 0.
fn resize_study(lab: &Lab, id: &str, payload: &str) -> ignorarium::runner::RunPlan {
    use ignorarium::backend::level0::Level0;
    let files = common::reserve_files(RESERVE);
    let before: Vec<(&str, &str)> = files.iter().map(|(p, t)| (&p[..], &t[..])).collect();
    register_study(lab, id, common::resize_plan(RESERVE), &before, payload);
    let root = common::work_of(&lab.home, id);
    ignorarium::runner::prepare_with(&lab.home, id, &root, vec![Box::new(Level0)]).unwrap()
}

/// The count the re-size gives a first batch whose every observation is `x`: the pooled
/// claim at the study's level, sized at -1 for the whole power over every cluster.
fn needed_at(
    plan: &ignorarium::runner::RunPlan,
    records: &[ignorarium::records::Record],
    x: f64,
) -> Option<u64> {
    use evidence::claim::{Bound, Statistic};
    use evidence::power::{Plan, blinded_sd, resize_count};
    let level = records
        .iter()
        .find_map(Register::of)
        .unwrap()
        .alpha
        .unwrap()
        .level;
    let p = Plan {
        stat: Statistic::Mean { lo: -2.0, hi: 2.0 },
        bounds: vec![Bound {
            tail: evidence::Tail::Lower,
            margin: 0.0,
        }],
        strata: vec![],
        level,
    };
    let s = blinded_sd(&[x.max(-2.0); FIRST]).unwrap();
    let cap = (FIRST + RESERVE) as u32;
    let seed = ignorarium::seed::seed(&plan.spec_hash, "resize/H1/");
    let sizes = vec![1; cap as usize];
    let n = resize_count(&p, &sizes, s, -1.0, 0.8, 4000, seed, cap).unwrap();
    n.map(u64::from)
}

/// The paths of the reserve inputs of `plan` by pin.
fn by_pin(plan: &ignorarium::runner::RunPlan) -> Vec<String> {
    use ignorarium::spec::model::Role;
    let mut held: Vec<(&str, &str)> = (plan.spec.inputs.iter())
        .filter(|i| i.role == Role::Reserve)
        .map(|i| (&i.sha256[..], &i.path[..]))
        .collect();
    held.sort();
    held.into_iter().map(|(_, p)| p.to_string()).collect()
}

/// The clusters of the reserve rows recorded in `records`, each once, in the order they ran,
/// and the count of those rows.
fn reserve_ran(
    plan: &ignorarium::runner::RunPlan,
    records: &[ignorarium::records::Record],
) -> (Vec<String>, usize) {
    let rows: Vec<&ignorarium::schedule::Row> = (records.iter())
        .filter(|r| r.kind == "row")
        .map(|r| &plan.schedule.rows[r.row.unwrap() as usize])
        .filter(|row| row.stage == "reserve")
        .collect();
    let mut clusters: Vec<String> = vec![];
    for row in &rows {
        if !clusters.contains(&row.cluster) {
            clusters.push(row.cluster.clone());
        }
    }
    (clusters, rows.len())
}

#[test]
fn no_reserve_row_runs_before_the_record() {
    use ignorarium::derived::{Resize, ResizeStratum};
    use ignorarium::runner::{RunStatus, run};
    let lab = lab();
    common::resize_pilot(&lab.home);
    // Every observation is 99 against 100: -1 %.
    let plan = resize_study(&lab, "grows", &flat("99"));
    let rows = plan.schedule.rows.len();
    assert_eq!(rows, 2 * (FIRST + RESERVE));
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let all = common::records_of(&lab, "grows");
    let resized: Vec<usize> = (0..all.len())
        .filter(|&i| all[i].kind == "resize")
        .collect();
    assert_eq!(resized.len(), 1);
    let at = resized[0];
    // Each first-batch row before the record, each reserve row after it.
    let stage =
        |r: &ignorarium::records::Record| plan.schedule.rows[r.row.unwrap() as usize].stage.clone();
    let before: Vec<String> = all[..at]
        .iter()
        .filter(|r| r.kind == "row")
        .map(stage)
        .collect();
    assert_eq!(before, vec!["confirmation".to_string(); 2 * FIRST]);
    let after = &all[at + 1..];
    assert!(
        after
            .iter()
            .filter(|r| r.kind == "row")
            .all(|r| stage(r) == "reserve")
    );
    // The count of the uncentred sd of the first batch, then the rest of it from the reserve,
    // by pin.
    let needed = needed_at(&plan, &all, 100.0 * (99.0f64 / 100.0 - 1.0));
    let appended = needed.unwrap() - FIRST as u64;
    assert!(appended > 0 && appended < RESERVE as u64, "{needed:?}");
    assert_eq!(
        Resize::of(&all[at]).unwrap(),
        Resize {
            strata: vec![ResizeStratum {
                stratum: String::new(),
                first: FIRST as u64,
                reserve: RESERVE as u64,
                needed,
                appended,
            }],
            decision: "continue".into(),
            reason: None,
        }
    );
    let (ran, count) = reserve_ran(&plan, after);
    assert_eq!(ran, by_pin(&plan)[..appended as usize].to_vec());
    assert_eq!(count, 2 * appended as usize);
}

#[test]
fn a_stopped_re_size_runs_no_reserve_row() {
    use ignorarium::derived::{Resize, ResizeStratum};
    use ignorarium::runner::run;
    let lab = lab();
    common::resize_pilot(&lab.home);
    // Every observation is -2 %: the spread of 2 needs more clusters than the 47 there are.
    let plan = resize_study(&lab, "short", &flat("98"));
    run(&plan).unwrap();
    let all = common::records_of(&lab, "short");
    assert_eq!(needed_at(&plan, &all, -2.0), None);
    let resized = kind_of(&lab, "short", "resize");
    assert_eq!(resized.len(), 1);
    assert_eq!(
        Resize::of(&resized[0]).unwrap(),
        Resize {
            strata: vec![ResizeStratum {
                stratum: String::new(),
                first: FIRST as u64,
                reserve: RESERVE as u64,
                needed: None,
                appended: 0,
            }],
            decision: "stop".into(),
            reason: Some("the reserve cannot reach the power the claim needs".into()),
        }
    );
    assert_eq!(reserve_ran(&plan, &all), (vec![], 0));
    assert_eq!(kind_of(&lab, "short", "row").len(), 2 * FIRST);
}

#[test]
fn a_resumed_run_writes_one_resize_record() {
    use ignorarium::derived::{RESIZE, Resize};
    use ignorarium::runner::{RunStatus, run};
    let lab = lab();
    common::resize_pilot(&lab.home);
    let plan = resize_study(&lab, "again", &flat("99"));
    run(&plan).unwrap();
    let dir = lab.home.study_dir("again");
    let log = dir.join("records.jsonl");
    let whole = std::fs::read_to_string(&log).unwrap();
    let all = common::records_of(&lab, "again");
    let at = all.iter().position(|r| r.kind == RESIZE).unwrap();
    let made = Resize::of(&all[at]).unwrap();
    let ran = reserve_ran(&plan, &all);
    // A box killed before the record, or after it and one reserve row, then resumed: one
    // record, the same decision, the same reserve rows and no other.
    for (n, kept) in [(0, at), (1, at + 2)] {
        let text: String = whole.split_inclusive('\n').take(kept).collect();
        std::fs::write(&log, text).unwrap();
        std::fs::rename(dir.join("run"), dir.join(format!("run-old-{n}"))).unwrap();
        assert_eq!(run(&plan).unwrap(), RunStatus::Done);
        let again = common::records_of(&lab, "again");
        let resized: Vec<_> = again.iter().filter(|r| r.kind == RESIZE).collect();
        assert_eq!(resized.len(), 1, "{n}");
        assert_eq!(Resize::of(resized[0]).unwrap(), made, "{n}");
        assert_eq!(reserve_ran(&plan, &again), ran, "{n}");
        assert_eq!(again.len(), all.len(), "{n}");
    }
    // A resume reads the record it finds and never makes it again: one that admits a single
    // reserve cluster runs that one alone.
    let text: String = whole.split_inclusive('\n').take(at).collect();
    std::fs::write(&log, text).unwrap();
    std::fs::rename(dir.join("run"), dir.join("run-old-2")).unwrap();
    let mut one = made.clone();
    one.strata[0].appended = 1;
    common::plant(
        &lab.home,
        "again",
        &[(RESIZE, serde_json::to_value(&one).unwrap())],
    );
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let again = common::records_of(&lab, "again");
    assert_eq!(again.iter().filter(|r| r.kind == RESIZE).count(), 1);
    assert_eq!(reserve_ran(&plan, &again), (by_pin(&plan)[..1].to_vec(), 2));
}

#[test]
fn a_pooled_re_size_takes_its_reserve_from_one_stratum() {
    use ignorarium::runner::{RunStatus, run};
    let lab = lab();
    common::resize_pilot(&lab.home);
    // The pooled claim's first batch spans fiction and 3 clusters in news; its reserve is
    // all in fiction, or with its first input moved to news, in both, named in input order.
    let news = |t: String| {
        let extra = format!("{}[pins]", common::clusters("news", 3));
        common::resize_plan(RESERVE)(t).replacen("[pins]", &extra, 1)
    };
    let fiction = "role = \"reserve\"\nstratum = \"fiction\"";
    let split = |t: String| news(t).replacen(fiction, "role = \"reserve\"\nstratum = \"news\"", 1);
    let dir = lab.root.join("plans");
    std::fs::create_dir_all(dir.join("data")).unwrap();
    for (p, text) in common::reserve_files(RESERVE) {
        std::fs::write(dir.join(p), text).unwrap();
    }
    let template = std::fs::read_to_string("../../templates/numeric-ablation.toml").unwrap();
    let mut checked = vec![];
    for (name, edit) in [("one", &news as &dyn Fn(String) -> String), ("two", &split)] {
        let path = dir.join(format!("{name}.toml"));
        std::fs::write(&path, edit(template.clone())).unwrap();
        checked.push(check_json(&lab, &path));
    }
    assert_eq!(
        (checked[0].0, codes(&checked[0].1)),
        (0, vec![]),
        "{}",
        checked[0].1
    );
    let (code, v) = &checked[1];
    assert_eq!((*code, codes(v)), (1, vec!["E-RSV-004"]), "{v}");
    let problem = (v["diagnostics"].as_array().unwrap().iter())
        .find(|d| d["code"] == "E-RSV-004")
        .map(|d| d["problem"].as_str().unwrap())
        .unwrap();
    assert!(
        problem.ends_with("A pooled claim's reserve spans strata news and fiction; list the strata in the claim, or keep its reserve in one stratum."),
        "{problem}"
    );
    // The accepted plan runs: one pooled count over the 18 first-batch clusters, then the
    // reserve clusters it appends, by pin, after them.
    let files = common::reserve_files(RESERVE);
    let before: Vec<(&str, &str)> = files.iter().map(|(p, t)| (&p[..], &t[..])).collect();
    register_study(&lab, "pooled", news, &before, &flat("99"));
    let root = common::work_of(&lab.home, "pooled");
    let level0: Box<dyn Backend> = Box::new(ignorarium::backend::level0::Level0);
    let plan = ignorarium::runner::prepare_with(&lab.home, "pooled", &root, vec![level0]).unwrap();
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let resize = resize_of(&lab, "pooled");
    assert_eq!(resize.decision, "continue");
    assert_eq!(resize.strata.len(), 1);
    let s = &resize.strata[0];
    assert_eq!(
        (s.stratum.as_str(), s.first, s.reserve),
        ("", 18, RESERVE as u64)
    );
    let appended = s.appended as usize;
    assert!(appended > 0 && appended < RESERVE, "{s:?}");
    let all = common::records_of(&lab, "pooled");
    assert_eq!(
        reserve_ran(&plan, &all),
        (by_pin(&plan)[..appended].to_vec(), 2 * appended)
    );
    let v = verdict_of(&lab, "pooled");
    assert_eq!(v.bounds[0].clusters, (18 + appended) as u64);
}

// ---- the end of a re-sized study ----

/// A payload that writes 100 for the reference and, for `arm-a`, 98.6 to 101.4 in steps of
/// 0.7 by the checksum of the unit: observations about 0 % with a spread near 1, so the
/// e-process never reaches its largest bet and every observation's place changes its value.
const VARIED: &str = "#!/bin/sh\ncase \"$2\" in exact) v=100;; *) h=$(printf '%s' \"$4\" | cksum | cut -d' ' -f1); case $((h % 5)) in 0) v=98.6;; 1) v=99.3;; 2) v=100;; 3) v=100.7;; *) v=101.4;; esac;; esac\nprintf '{\"ppl\": %s}' \"$v\" > \"$IGNORARIUM_OUT/values.json\"\n";

/// `flat("99")`, but the `arm-a` row of the first batch's cluster `data/fiction-03.txt` fails.
const ONE_FAILS: &str = "#!/bin/sh\ncase \"$2\" in exact) v=100;; *) case \"$4\" in data/fiction-03.txt*) exit 3;; esac; v=99;; esac\nprintf '{\"ppl\": %s}' \"$v\" > \"$IGNORARIUM_OUT/values.json\"\n";

/// Each cluster's observation in the `ok` row records of `records`: arm-a's percent change
/// over the reference on its one unit, by path.
fn observed(
    plan: &ignorarium::runner::RunPlan,
    records: &[ignorarium::records::Record],
) -> std::collections::BTreeMap<String, f64> {
    let mut values = std::collections::BTreeMap::new();
    for r in records
        .iter()
        .filter(|r| r.kind == "row" && r.body["exit"] == "ok")
    {
        let row = &plan.schedule.rows[r.row.unwrap() as usize];
        let v = r.body["values"]["ppl"].as_f64().unwrap();
        values.insert((row.cluster.clone(), row.condition.clone()), v);
    }
    (values.iter())
        .filter(|((_, c), _)| c == "arm-a")
        .map(|((p, _), a)| {
            let reference = values[&(p.clone(), "exact".to_string())];
            (p.clone(), 100.0 * (a / reference - 1.0))
        })
        .collect()
}

/// The paths of the first batch's clusters, in the order the analysis reads them.
fn first_batch(plan: &ignorarium::runner::RunPlan) -> Vec<String> {
    use ignorarium::spec::model::Role;
    let all = ignorarium::units::clusters(&plan.spec, &plan.spec_hash);
    (ignorarium::analysis::distinct_clusters(all, &plan.inputs).into_iter())
        .filter(|c| c.role == Role::Confirmation)
        .map(|c| c.path)
        .collect()
}

/// The e-value of the study's claim, below 0 by a mean in [-2, 2], over `obs` in order.
fn e_of(obs: &[f64]) -> f64 {
    use evidence::claim::{Bound, Statistic, e_value};
    let below = Bound {
        tail: evidence::Tail::Lower,
        margin: 0.0,
    };
    e_value(&Statistic::Mean { lo: -2.0, hi: 2.0 }, &below, obs).unwrap()
}

/// The study's one `resize` record.
fn resize_of(lab: &Lab, id: &str) -> ignorarium::derived::Resize {
    let rs = kind_of(lab, id, "resize");
    assert_eq!(rs.len(), 1);
    ignorarium::derived::Resize::of(&rs[0]).unwrap()
}

/// The study's one `verdict` record.
fn verdict_of(lab: &Lab, id: &str) -> ignorarium::derived::Verdict {
    let vs = kind_of(lab, id, "verdict");
    assert_eq!(vs.len(), 1);
    ignorarium::derived::Verdict::of(&vs[0]).unwrap()
}

/// The study's register record.
fn register_of(lab: &Lab, id: &str) -> Register {
    Register::of(&kind_of(lab, id, "register")[0]).unwrap()
}

/// The plan log the study registered in.
fn plan_log_of(lab: &Lab, id: &str) -> ignorarium::planlog::store::PlanLog {
    let log = register_of(lab, id).log;
    ignorarium::planlog::store::PlanLog::open(&lab.home.root.join(log)).unwrap()
}

/// The `release` leaves of the plan log the study registered in.
fn releases(lab: &Lab, id: &str) -> Vec<ignorarium::planlog::store::Leaf> {
    let leaves = plan_log_of(lab, id).leaves().unwrap();
    leaves.into_iter().filter(|l| l.kind == "release").collect()
}

/// The `(exact, normalized)` hashes of the reserve inputs at `paths`, sorted.
fn reserve_data(paths: &[String]) -> Vec<(String, String)> {
    let mut data: Vec<(String, String)> = (paths.iter())
        .map(|p| ignorarium::prior::file_hashes(format!("{p}\n").as_bytes()))
        .collect();
    data.sort();
    data
}

/// The `stream.close` body of the study's slot.
fn close_of_slot(lab: &Lab, id: &str) -> Value {
    use ignorarium::stream::{Log, LogName};
    let alpha = register_of(lab, id).alpha.unwrap();
    let key = keys::load(&lab.home).unwrap().unwrap();
    let log = Log::of(&lab.home, LogName::parse(&alpha.log).unwrap());
    let all = ignorarium::records::read_all(log.path(), &key.verifying_key()).unwrap();
    let closes: Vec<Value> = (all.into_iter())
        .filter(|r| r.kind == "stream.close" && r.body["study"] == id)
        .map(|r| r.body)
        .collect();
    assert_eq!(closes.len(), 1, "{closes:?}");
    closes[0].clone()
}

#[test]
fn finish_waits_for_admitted_rows_only() {
    use ignorarium::analysis::finish;
    use ignorarium::runner::{RunStatus, run};
    let lab = lab();
    common::resize_pilot(&lab.home);
    let plan = resize_study(&lab, "waits", &flat("99"));
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let all = common::records_of(&lab, "waits");
    let (ran, rows) = reserve_ran(&plan, &all);
    // The analysis follows the rows the record admits; the other reserve rows never ran.
    assert!(!ran.is_empty() && ran.len() < RESERVE, "{ran:?}");
    assert_eq!(kind_of(&lab, "waits", "row").len(), 2 * FIRST + rows);
    assert_eq!(kind_of(&lab, "waits", "analysis").len(), 1);
    assert_eq!(kind_of(&lab, "waits", "verdict").len(), 1);
    let log = lab.home.study_dir("waits").join("records.jsonl");
    let whole = std::fs::read_to_string(&log).unwrap();
    let resized = all.iter().position(|r| r.kind == "resize").unwrap();
    let last_row = all.iter().rposition(|r| r.kind == "row").unwrap();
    // Without the record, or without its last admitted row, finish waits.
    for kept in [resized, last_row] {
        let text: String = whole.split_inclusive('\n').take(kept).collect();
        std::fs::write(&log, text).unwrap();
        finish(&plan).unwrap();
        assert_eq!(common::records_of(&lab, "waits").len(), kept, "{kept}");
    }
    // With every admitted row, it adds the analysis and its verdict.
    let text: String = whole.split_inclusive('\n').take(last_row + 1).collect();
    std::fs::write(&log, text).unwrap();
    finish(&plan).unwrap();
    assert_eq!(kind_of(&lab, "waits", "analysis").len(), 1);
    assert_eq!(kind_of(&lab, "waits", "verdict").len(), 1);
}

#[test]
fn the_e_process_sees_the_first_batch_then_the_reserve_in_pin_order() {
    use evidence::rng::Rng;
    use ignorarium::derived::Estimate;
    use ignorarium::runner::{RunStatus, run};
    let lab = lab();
    common::resize_pilot(&lab.home);
    let plan = resize_study(&lab, "order", VARIED);
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let all = common::records_of(&lab, "order");
    let x = observed(&plan, &all);
    let appended = resize_of(&lab, "order").strata[0].appended as usize;
    assert!(appended > 0 && appended < RESERVE, "{appended}");
    // The first batch in the order the test seed draws, then the reserve by pin.
    let first: Vec<f64> = first_batch(&plan).iter().map(|p| x[p]).collect();
    assert_eq!(first.len(), FIRST);
    let path = ignorarium::seed::seed(&plan.spec_hash, "test/H1/");
    let order = evidence::claim::order(FIRST, &mut Rng::from_seed(path));
    let drawn: Vec<f64> = order.iter().map(|&i| first[i]).collect();
    let reserve: Vec<f64> = by_pin(&plan)[..appended].iter().map(|p| x[p]).collect();
    let e = e_of(&[&drawn[..], &reserve[..]].concat());
    let v = verdict_of(&lab, "order");
    assert_eq!(v.e_value, Some(e));
    assert_eq!(v.bounds.len(), 1);
    assert_eq!(v.bounds[0].clusters, (FIRST + appended) as u64);
    assert_eq!(v.bounds[0].value, Some(e));
    // Another order gives another e-value, so the one the verdict holds is this order's.
    let reversed: Vec<f64> = reserve.iter().rev().copied().collect();
    assert_ne!(e, e_of(&[&drawn[..], &reversed[..]].concat()));
    assert_ne!(e, e_of(&[&first[..], &reserve[..]].concat()));
    assert_ne!(e, e_of(&[&reserve[..], &drawn[..]].concat()));
    // The estimate holds the appended clusters too.
    let est = kind_of(&lab, "order", "estimate");
    assert_eq!(est.len(), 1);
    let est = Estimate::of(&est[0]).unwrap();
    assert_eq!(
        (est.clusters, est.units),
        ((FIRST + appended) as u64, (FIRST + appended) as u64)
    );
}

#[test]
fn a_stopped_study_fails_its_claims_with_e_value_zero_and_closes_its_slot_with_zero() {
    use ignorarium::runner::{RunStatus, run};
    let lab = lab();
    common::resize_pilot(&lab.home);
    // The reason is the resize record's, after `stopped: `: a reserve too short for a spread
    // of 2, or a first-batch unit with no value.
    let short = "the reserve cannot reach the power the claim needs";
    let broken = "incomplete units in the first batch: 1";
    for (id, payload, why) in [
        ("short", flat("98"), short),
        ("broken", ONE_FAILS.into(), broken),
    ] {
        let plan = resize_study(&lab, id, &payload);
        assert_eq!(run(&plan).unwrap(), RunStatus::Done, "{id}");
        let resize = resize_of(&lab, id);
        assert_eq!(
            (resize.decision.as_str(), resize.reason.as_deref()),
            ("stop", Some(why))
        );
        let all = common::records_of(&lab, id);
        assert_eq!(reserve_ran(&plan, &all), (vec![], 0), "{id}");
        assert_eq!(kind_of(&lab, id, "row").len(), 2 * FIRST, "{id}");
        let v = verdict_of(&lab, id);
        let level = register_of(&lab, id).alpha.unwrap().level;
        assert_eq!(
            (v.verdict.as_str(), v.basis.as_str(), v.level, v.e_value),
            ("fail", "test", Some(level), Some(0.0)),
            "{id}"
        );
        assert_eq!(v.reason, Some(format!("stopped: {why}")), "{id}");
        // The first batch alone was analysed.
        let complete = FIRST as u64 - u64::from(id == "broken");
        assert_eq!(v.bounds[0].clusters, complete, "{id}");
        let close = close_of_slot(&lab, id);
        assert_eq!(
            (&close["e_value"], &close["discovery"]),
            (&serde_json::json!(0.0), &serde_json::json!(false)),
            "{id}"
        );
    }
}

/// The study's analysis record, which comes after its last row and its resize record; that
/// record names no sd.
fn analysis_after_the_end(lab: &Lab, id: &str) -> ignorarium::derived::Analysis {
    let all = common::records_of(lab, id);
    let at = all.iter().position(|r| r.kind == "analysis").unwrap();
    let last_row = all.iter().rposition(|r| r.kind == "row").unwrap();
    let resized = all.iter().position(|r| r.kind == "resize").unwrap();
    assert!(at > last_row && at > resized, "{at}");
    assert!(!all[resized].body.to_string().contains("sd"));
    ignorarium::derived::Analysis::of(&all[at]).unwrap()
}

#[test]
fn the_analysis_record_holds_the_resize_sd_and_the_appended_sd_after_the_end() {
    use evidence::power::blinded_sd;
    use ignorarium::derived::GroupSd;
    use ignorarium::runner::{RunStatus, run};
    let sd = |s: Option<f64>| {
        vec![GroupSd {
            hypothesis: "H1".into(),
            stratum: String::new(),
            sd: s,
        }]
    };
    let lab = lab();
    common::resize_pilot(&lab.home);
    let plan = resize_study(&lab, "spread", VARIED);
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let x = observed(&plan, &common::records_of(&lab, "spread"));
    let appended = resize_of(&lab, "spread").strata[0].appended as usize;
    let first: Vec<f64> = first_batch(&plan).iter().map(|p| x[p]).collect();
    let reserve: Vec<f64> = by_pin(&plan)[..appended].iter().map(|p| x[p]).collect();
    let a = analysis_after_the_end(&lab, "spread");
    assert_eq!(a.resize_sd, Some(sd(blinded_sd(&first))));
    assert_eq!(a.appended_sd, Some(sd(blinded_sd(&reserve))));
    assert_ne!(blinded_sd(&first), blinded_sd(&reserve));
    // A stopped study appended none: its sd is null. It runs in a box of its own, since the
    // first study used part of the reserve.
    let lab = common::lab();
    common::resize_pilot(&lab.home);
    let plan = resize_study(&lab, "flat", &flat("98"));
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let a = analysis_after_the_end(&lab, "flat");
    assert_eq!(a.resize_sd, Some(sd(Some(2.0))));
    assert_eq!(a.appended_sd, Some(sd(None)));
    let body = &kind_of(&lab, "flat", "analysis")[0].body;
    assert_eq!(body["appended_sd"][0]["sd"], Value::Null);
}

/// A payload that writes 100 for the reference and, for `arm-a`, 97, 103 or 99 by the
/// checksum of the unit: -3 % lies below the range [-2, 2], +3 % above it, -1 % inside.
const CLIPPED: &str = "#!/bin/sh\ncase \"$2\" in exact) v=100;; *) h=$(printf '%s' \"$4\" | cksum | cut -d' ' -f1); case $((h % 4)) in 0) v=97;; 1) v=103;; *) v=99;; esac;; esac\nprintf '{\"ppl\": %s}' \"$v\" > \"$IGNORARIUM_OUT/values.json\"\n";

#[test]
fn the_analysis_record_and_status_count_the_units_the_range_clipped() {
    use ignorarium::derived::GroupClip;
    use ignorarium::runner::{RunStatus, run};
    let lab = lab();
    common::resize_pilot(&lab.home);
    let plan = resize_study(&lab, "clip", CLIPPED);
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    // The units the test read: the first batch, then the reserve clusters it appended.
    let x = observed(&plan, &common::records_of(&lab, "clip"));
    let appended = resize_of(&lab, "clip").strata[0].appended as usize;
    let read: Vec<f64> = (first_batch(&plan).iter().chain(&by_pin(&plan)[..appended]))
        .map(|p| x[p])
        .collect();
    let count = |f: fn(f64) -> bool| read.iter().filter(|&&v| f(v)).count() as u64;
    let (below, above) = (count(|v| v < -2.0), count(|v| v > 2.0));
    assert!(below > 0 && above > 0 && below + above < read.len() as u64);
    let a = analysis_after_the_end(&lab, "clip");
    let want = GroupClip {
        hypothesis: "H1".into(),
        stratum: String::new(),
        below,
        above,
        units: read.len() as u64,
    };
    assert_eq!(a.clipped, [want]);
    // Status shows them next to the verdict.
    let verdict = kind_of(&lab, "clip", "verdict")[0].body["verdict"].clone();
    let s = status(&lab, "clip");
    assert_eq!(
        s["verdicts"],
        serde_json::json!([{
            "hypothesis": "H1", "verdict": verdict,
            "clipped": [{ "stratum": "", "below": below, "above": above, "units": read.len() }],
        }])
    );
    let a = bin(&lab).args(["status", "clip"]).assert().success();
    let want = format!(
        "verdict H1: {}\nclipped H1 pooled: {below} below, {above} above, of {} units\n",
        verdict.as_str().unwrap(),
        read.len()
    );
    assert!(text(&a).contains(&want), "{}", text(&a));
    // The same log, signed again with an analysis record written before the count existed:
    // it verifies, and status prints the verdict with no count.
    let key = keys::load(&lab.home).unwrap().unwrap();
    let log = lab.home.study_dir("clip").join("records.jsonl");
    let all = common::records_of(&lab, "clip");
    std::fs::rename(&log, log.with_extension("before")).unwrap();
    for mut r in all {
        if r.kind == "analysis" {
            r.body.as_object_mut().unwrap().remove("clipped").unwrap();
        }
        let draft = ignorarium::records::RecordDraft {
            study: r.study,
            row: r.row,
            kind: r.kind,
            body: r.body,
        };
        ignorarium::records::append(&log, &key, draft).unwrap();
    }
    let s = status(&lab, "clip");
    assert_eq!(s["verdicts"][0]["verdict"], verdict);
    assert_eq!(s["verdicts"][0]["clipped"], serde_json::json!([]));
    let a = bin(&lab).args(["status", "clip"]).assert().success();
    let want = format!("verdict H1: {}\n", verdict.as_str().unwrap());
    assert!(text(&a).contains(&want), "{}", text(&a));
    assert!(!text(&a).contains("clipped"), "{}", text(&a));
    // A hypothesis with no claim clips nothing, so status prints no count for it.
    plain(&lab, "none");
    bin_at(&lab, "none").args(["run", "none"]).assert().code(0);
    let s = status(&lab, "none");
    assert_eq!(s["verdicts"][0]["clipped"], serde_json::json!([]));
    let a = bin(&lab).args(["status", "none"]).assert().success();
    assert!(text(&a).contains("verdict H1: none\n"), "{}", text(&a));
    assert!(!text(&a).contains("clipped"), "{}", text(&a));
}

#[test]
fn unused_reserve_data_is_released_and_fresh_again() {
    use ignorarium::runner::{RunStatus, run};
    let lab = lab();
    common::resize_pilot(&lab.home);
    let plan = resize_study(&lab, "grows", &flat("99"));
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let appended = resize_of(&lab, "grows").strata[0].appended as usize;
    let pins = by_pin(&plan);
    // One release names the plan leaf by its commitment and every reserve input not appended.
    let register = register_of(&lab, "grows");
    let leaf = plan_log_of(&lab, "grows")
        .leaf(register.leaf)
        .unwrap()
        .unwrap();
    let released = releases(&lab, "grows");
    assert_eq!(released.len(), 1);
    let r = &released[0];
    assert_eq!(
        (r.label.as_str(), &r.commitment, &r.family, &r.mode),
        ("study:grows", &leaf.commitment, &None, &None)
    );
    assert!(leaf.commitment.is_some());
    let mut data = r.data_hashes.clone();
    data.sort();
    assert_eq!(data, reserve_data(&pins[appended..]));
    // A later confirmatory plan that names a released text passes E-CNF-005; one that names
    // an appended text fails it.
    let later = |n: usize, path: &str| {
        let root = lab.root.join(format!("later-{n}"));
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/confirm.txt"), format!("{path}\n")).unwrap();
        let t = std::fs::read_to_string("../../templates/numeric-ablation.toml").unwrap();
        let spec = root.join("spec.toml");
        std::fs::write(&spec, common::confirmatory(t)).unwrap();
        let (_, v) = check_json(&lab, &spec);
        codes(&v).contains(&"E-CNF-005")
    };
    assert!(!later(0, &pins[appended]), "a released text is fresh");
    assert!(!later(1, &pins[RESERVE - 1]), "a released text is fresh");
    assert!(later(2, &pins[0]), "an appended text is used");
    assert!(later(3, &pins[appended - 1]), "an appended text is used");
}

#[test]
fn finish_twice_appends_one_release() {
    use ignorarium::analysis::finish;
    use ignorarium::runner::{RunStatus, run};
    let lab = lab();
    common::resize_pilot(&lab.home);
    // A stopped study releases its whole reserve.
    let plan = resize_study(&lab, "twice", &flat("98"));
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let once = releases(&lab, "twice");
    assert_eq!(once.len(), 1);
    let mut data = once[0].data_hashes.clone();
    data.sort();
    assert_eq!(data, reserve_data(&by_pin(&plan)));
    let leaves = plan_log_of(&lab, "twice").leaves().unwrap().len();
    finish(&plan).unwrap();
    assert_eq!(releases(&lab, "twice"), once);
    assert_eq!(plan_log_of(&lab, "twice").leaves().unwrap().len(), leaves);
    // A finish that adds the verdict again still adds no release.
    let log = lab.home.study_dir("twice").join("records.jsonl");
    let whole = std::fs::read_to_string(&log).unwrap();
    let kept = whole.lines().count() - 1;
    let text: String = whole.split_inclusive('\n').take(kept).collect();
    std::fs::write(&log, text).unwrap();
    finish(&plan).unwrap();
    assert_eq!(kind_of(&lab, "twice", "verdict").len(), 1);
    assert_eq!(releases(&lab, "twice"), once);
}

/// Every key of `v`, at any depth.
fn keys_of(v: &Value) -> Vec<String> {
    match v {
        Value::Object(m) => (m.iter())
            .flat_map(|(k, v)| std::iter::once(k.clone()).chain(keys_of(v)))
            .collect(),
        Value::Array(a) => a.iter().flat_map(keys_of).collect(),
        _ => vec![],
    }
}

#[test]
fn status_shows_counts_and_decision_only() {
    use ignorarium::runner::{RunStatus, run};
    // What status may not show: a spread, a mean, an e-value, the reserve size or a reason.
    let hidden = [
        "sd",
        "resize_sd",
        "appended_sd",
        "mean",
        "e_value",
        "value",
        "reserve",
        "reason",
    ];
    let words = |t: &str| -> Vec<String> {
        (t.split(|c: char| !c.is_ascii_alphanumeric() && c != '_'))
            .map(String::from)
            .collect()
    };
    let lab = lab();
    common::resize_pilot(&lab.home);
    let plan = resize_study(&lab, "counts", VARIED);
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let r = resize_of(&lab, "counts");
    let g = &r.strata[0];
    let needed = g.needed.unwrap();
    let s = status(&lab, "counts");
    assert_eq!(
        s["resize"],
        serde_json::json!({
            "decision": "continue",
            "strata": [{ "stratum": "", "first": FIRST, "needed": needed, "appended": g.appended }],
        })
    );
    for k in keys_of(&s) {
        assert!(!hidden.contains(&k.as_str()), "{k}");
    }
    // The rows that run are all the rows: the reserve rows the record leaves out count not.
    let rows = 2 * (FIRST as u64 + g.appended);
    let done = |s: &Value| {
        (
            s["done"].clone(),
            s["total"].clone(),
            s["eta_seconds"].clone(),
        )
    };
    let ended = |n: u64| {
        (
            serde_json::json!(n),
            serde_json::json!(n),
            serde_json::json!(0.0),
        )
    };
    assert_eq!(done(&s), ended(rows));
    let a = bin(&lab).args(["status", "counts"]).assert().success();
    let t = text(&a);
    let want = format!(
        "resize: continue\nresize pooled: first {FIRST}, needed {needed}, appended {}\n",
        g.appended
    );
    assert!(t.contains(&want), "{t}");
    for w in words(&t) {
        assert!(!hidden.contains(&w.as_str()), "{w} in {t}");
    }
    // A study with no re-size shows none.
    plain(&lab, "none");
    assert_eq!(status(&lab, "none")["resize"], Value::Null);
    let a = bin(&lab).args(["status", "none"]).assert().success();
    assert!(!text(&a).contains("resize"));
    // A stopped study's group needs no count. It runs in a box of its own, since the first
    // study used part of the reserve.
    let lab = common::lab();
    common::resize_pilot(&lab.home);
    let plan = resize_study(&lab, "halted", &flat("98"));
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let s = status(&lab, "halted");
    assert_eq!(s["resize"]["decision"], "stop");
    assert_eq!(s["resize"]["strata"][0]["needed"], Value::Null);
    assert_eq!(done(&s), ended(2 * FIRST as u64));
    let a = bin(&lab).args(["status", "halted"]).assert().success();
    let want = format!("resize: stop\nresize pooled: first {FIRST}, needed n/a, appended 0\n");
    assert!(text(&a).contains(&want), "{}", text(&a));
}
