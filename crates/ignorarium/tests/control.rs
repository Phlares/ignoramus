mod common;

use common::{
    Lab, PAYLOAD, events_of, indices, kind_of, lab, lines_of, named, plain, register_study,
    rows_of, with_payload,
};
use ignorarium::backend::level0::Level0;
use ignorarium::backend::{Backend, Exit, Probe, RowJob, RowOutcome, Watch};
use ignorarium::control::{self, ControlFile, DeviationRow, RunLock, Signal, State, Timing};
use ignorarium::home::Home;
use ignorarium::keys;
use ignorarium::records::Record;
use ignorarium::runner::{self, HEARTBEAT, RunError, RunPlan, RunStatus, prepare_with};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const PAUSED: &str = "a pause was requested";

/// Row 1 marks itself started, then takes half a second.
const SLOW_ROW_1: &str = r#"#!/bin/sh
if [ "$IGNORARIUM_ROW" = 1 ]; then
  echo started > "$IGNORARIUM_OUT/started"
  sleep 0.5
fi
echo ok > "$IGNORARIUM_OUT/result"
"#;

/// Row 2 marks itself started, leaves a child that writes `late` after 1.5 s, and sleeps 30 s.
const SLOW_ROW_2: &str = r#"#!/bin/sh
if [ "$IGNORARIUM_ROW" = 2 ]; then
  (sleep 1.5; echo late > "$IGNORARIUM_OUT/late") &
  echo started > "$IGNORARIUM_OUT/started"
  sleep 30
fi
echo ok > "$IGNORARIUM_OUT/result"
"#;

fn plan_of(lab: &Lab, id: &str) -> RunPlan {
    prepare_with(&lab.home, id, vec![Box::new(Level0)]).unwrap()
}

fn control_of(lab: &Lab, id: &str) -> ControlFile {
    ControlFile::of(&lab.home, id)
}

fn go(plan: &RunPlan) -> Result<RunStatus, RunError> {
    runner::run(plan, &ControlFile::of(&plan.home, &plan.study_id))
}

/// A stop sent the way the `stop` command sends it: the request, then the trace when no run
/// holds the lock.
fn send_stop(home: &Home, id: &str, why: &str) -> Option<Record> {
    let key = keys::load(home).unwrap().unwrap();
    let c = ControlFile::of(home, id);
    c.request_stop(why).unwrap();
    control::stop_if_idle(home, id, &key, &c).unwrap()
}

/// On another thread, as another process would: wait until row `row` writes `started`, then act.
fn once_started(
    home: &Home,
    id: &str,
    row: u64,
    act: impl FnOnce(&Home) + Send + 'static,
) -> JoinHandle<()> {
    let home = home.clone();
    let marker = home
        .study_dir(id)
        .join(format!("run/rows/{row}/out/started"));
    thread::spawn(move || {
        let t = Instant::now();
        while !marker.exists() {
            assert!(
                t.elapsed() < Duration::from_secs(20),
                "row {row} never started"
            );
            thread::sleep(Duration::from_millis(5));
        }
        act(&home);
    })
}

fn state_of(lab: &Lab, id: &str) -> Option<State> {
    State::read(&lab.home, id).unwrap()
}

fn deviations_of(lab: &Lab, id: &str) -> Vec<Value> {
    lines_of(&lab.home.study_dir(id), "deviations.jsonl")
}

/// The OS user name, as `id` prints it, else the uid number.
fn os_user() -> String {
    let id = |arg: &str| {
        std::process::Command::new("id")
            .arg(arg)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8(o.stdout).unwrap().trim().to_string())
    };
    id("-un").or_else(|| id("-u")).unwrap()
}

#[test]
fn heartbeat_is_fifteen_seconds() {
    assert_eq!(HEARTBEAT, Duration::from_secs(15));
    let lab = lab();
    let dir = plain(&lab, "hb0");
    let plan = plan_of(&lab, "hb0");
    assert_eq!(plan.heartbeat, HEARTBEAT);
    assert_eq!(go(&plan).unwrap(), RunStatus::Done);
    // Short rows at the default cadence: each row starts and ends, with no heartbeat.
    let ev = events_of(&dir);
    assert_eq!(named(&ev, "row.start").len(), 6);
    assert_eq!(named(&ev, "row.end").len(), 6);
    assert!(named(&ev, "heartbeat").is_empty());
}

#[test]
fn heartbeat_fires_on_the_cadence() {
    let lab = lab();
    let dir = with_payload(&lab, "hb", "#!/bin/sh\nsleep 0.3\n");
    let mut plan = plan_of(&lab, "hb");
    plan.heartbeat = Duration::from_millis(50);
    assert_eq!(go(&plan).unwrap(), RunStatus::Done);
    let ev = events_of(&dir);
    for r in rows_of(&lab, "hb") {
        let i = r.row.unwrap();
        let of_row = |name: &str| -> Vec<(usize, &Value)> {
            ev.iter()
                .enumerate()
                .filter(|(_, e)| e["name"] == name && e["attrs"]["row"] == i)
                .collect()
        };
        let (start, end, beats) = (of_row("row.start"), of_row("row.end"), of_row("heartbeat"));
        assert_eq!((start.len(), end.len()), (1, 1), "row {i}");
        let span = start[0].1["span_id"].as_str().unwrap();
        assert_eq!(span.len(), 16);
        assert_eq!(end[0].1["span_id"], span);
        assert_eq!(end[0].1["attrs"]["exit"], "ok");
        let wall = r.body["wall_seconds"].as_f64().unwrap();
        let most = (wall / 0.05) as usize + 1;
        assert!(
            (2..=most).contains(&beats.len()),
            "row {i}: {} heartbeats in {wall} s",
            beats.len()
        );
        for (at, b) in &beats {
            assert_eq!(b["span_id"], span);
            assert_eq!(b["level"], "info");
            assert!(start[0].0 < *at && *at < end[0].0);
        }
    }
    let spans: BTreeSet<&str> = named(&ev, "row.start")
        .iter()
        .map(|e| e["span_id"].as_str().unwrap())
        .collect();
    assert_eq!(spans.len(), 6);
}

#[test]
fn stop_mid_row_kills_the_group_and_the_row_gets_no_record() {
    let lab = lab();
    let dir = with_payload(&lab, "mid", SLOW_ROW_2);
    let plan = plan_of(&lab, "mid");
    let other = once_started(&lab.home, "mid", 2, |home| {
        send_stop(home, "mid", "manual");
    });
    let t = Instant::now();
    assert_eq!(go(&plan).unwrap(), RunStatus::Stopped("manual".into()));
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
    other.join().unwrap();
    assert_eq!(indices(&lab, "mid"), vec![0, 1]);
    let stops = kind_of(&lab, "mid", "stop");
    assert_eq!(stops.len(), 1);
    assert_eq!(stops[0].body["why"], "manual");
    assert_eq!(stops[0].body["rows_done"], 2);
    let devs = deviations_of(&lab, "mid");
    assert_eq!(devs.len(), 1);
    assert_eq!(devs[0]["timing"], "during-collection");
    assert_eq!(state_of(&lab, "mid"), Some(State::Stopped("manual".into())));
    let ev = events_of(&dir);
    assert_eq!(named(&ev, "run.stop").len(), 1);
    let ends = named(&ev, "row.end");
    assert_eq!(ends.len(), 3);
    assert_eq!(ends[2]["attrs"], json!({"row": 2, "exit": "stopped"}));
    // No later row starts, and the row's whole group is dead: its child never writes.
    assert!(!dir.join("run/rows/3").exists());
    thread::sleep(Duration::from_millis(2000));
    assert!(dir.join("run/rows/2/out/started").is_file());
    assert!(!dir.join("run/rows/2/out/late").exists());
}

#[test]
fn stop_with_no_live_run_writes_one_record_and_one_deviation_row() {
    let lab = lab();
    let dir = plain(&lab, "idle");
    let rec = send_stop(&lab.home, "idle", "abandon").expect("no run holds the lock");
    assert_eq!(rec.kind, "stop");
    assert_eq!(rec.row, None);
    let who = os_user();
    let b = &rec.body;
    let when = b["when"].as_str().unwrap().to_string();
    assert_eq!(
        *b,
        json!({"who": who, "when": when, "why": "abandon", "label_before": "signal",
               "label_after": "signal", "rows_done": 0})
    );
    assert!(
        when.len() == 24 && when.ends_with('Z') && when <= rec.ts,
        "{when} {}",
        rec.ts
    );
    assert_eq!(kind_of(&lab, "idle", "stop").len(), 1);
    assert_eq!(
        deviations_of(&lab, "idle"),
        vec![json!({
            "kind": "stop",
            "reason": "abandon",
            "timing": "before-collection",
            "original": "run every row of the schedule",
            "justification": format!("break-glass stop by {who}"),
            "impact": "0 rows ran before the stop; the earnable label drops from signal to signal",
        })]
    );
    assert_eq!(
        state_of(&lab, "idle"),
        Some(State::Stopped("abandon".into()))
    );
    let ev = events_of(&dir);
    let stop_ev = named(&ev, "run.stop");
    assert_eq!(stop_ev.len(), 1);
    assert_eq!(
        stop_ev[0]["attrs"],
        json!({"why": "abandon", "rows_done": 0})
    );
    assert_eq!(stop_ev[0]["level"], "warn");
    // The trace id is the study's: the first 16 bytes of the spec hash.
    let plan = plan_of(&lab, "idle");
    assert_eq!(stop_ev[0]["trace_id"], hex::encode(&plan.spec_hash[..16]));
    // The study never runs a row after the stop.
    assert_eq!(go(&plan).unwrap(), RunStatus::Stopped("abandon".into()));
    assert!(rows_of(&lab, "idle").is_empty());
    assert!(!dir.join("run/rows").exists());
    assert_eq!(kind_of(&lab, "idle", "stop").len(), 1);
    assert_eq!(deviations_of(&lab, "idle").len(), 1);
    assert_eq!(named(&events_of(&dir), "run.stop").len(), 1);
}

#[test]
fn two_stops_write_one_record() {
    let lab = lab();
    let dir = plain(&lab, "twice");
    let first = send_stop(&lab.home, "twice", "first").unwrap();
    assert_eq!(send_stop(&lab.home, "twice", "second"), Some(first.clone()));
    // The lock holder's own second call writes nothing either.
    let key = keys::load(&lab.home).unwrap().unwrap();
    let lock = RunLock::try_take(&lab.home, "twice").unwrap().unwrap();
    let again = control::stop(&lab.home, "twice", &key, "third", &lock).unwrap();
    assert_eq!(again, first);
    assert_eq!(kind_of(&lab, "twice", "stop").len(), 1);
    assert_eq!(deviations_of(&lab, "twice").len(), 1);
    assert_eq!(named(&events_of(&dir), "run.stop").len(), 1);
    assert_eq!(
        state_of(&lab, "twice"),
        Some(State::Stopped("first".into()))
    );
}

#[test]
fn stop_after_pause_still_writes_the_stop_trace() {
    let lab = lab();
    plain(&lab, "ps");
    let plan = plan_of(&lab, "ps");
    control_of(&lab, "ps").request_pause().unwrap();
    assert_eq!(go(&plan).unwrap(), RunStatus::Paused(PAUSED.into()));
    assert!(rows_of(&lab, "ps").is_empty());
    assert_eq!(state_of(&lab, "ps"), Some(State::Paused(PAUSED.into())));
    assert!(send_stop(&lab.home, "ps", "manual").is_some());
    assert_eq!(kind_of(&lab, "ps", "stop").len(), 1);
    assert_eq!(deviations_of(&lab, "ps").len(), 1);
    assert_eq!(state_of(&lab, "ps"), Some(State::Stopped("manual".into())));
}

#[test]
fn pause_then_stops_while_a_row_runs_stop_once() {
    let lab = lab();
    let dir = with_payload(&lab, "race", SLOW_ROW_2);
    let plan = plan_of(&lab, "race");
    let other = once_started(&lab.home, "race", 2, |home| {
        let c = ControlFile::of(home, "race");
        c.request_pause().unwrap();
        send_stop(home, "race", "manual");
        send_stop(home, "race", "again");
        c.request_pause().unwrap();
    });
    assert_eq!(go(&plan).unwrap(), RunStatus::Stopped("manual".into()));
    other.join().unwrap();
    assert_eq!(indices(&lab, "race"), vec![0, 1]);
    assert_eq!(kind_of(&lab, "race", "stop").len(), 1);
    assert_eq!(deviations_of(&lab, "race").len(), 1);
    let ev = events_of(&dir);
    assert_eq!(named(&ev, "run.stop").len(), 1);
    assert!(named(&ev, "run.pause").is_empty());
    assert_eq!(
        state_of(&lab, "race"),
        Some(State::Stopped("manual".into()))
    );
}

#[test]
fn pause_then_resume_continues_under_the_same_hash() {
    let lab = lab();
    let dir = with_payload(&lab, "pr", SLOW_ROW_1);
    let plan = plan_of(&lab, "pr");
    let other = once_started(&lab.home, "pr", 1, |home| {
        ControlFile::of(home, "pr").request_pause().unwrap();
    });
    assert_eq!(go(&plan).unwrap(), RunStatus::Paused(PAUSED.into()));
    other.join().unwrap();
    // A pause never kills a row: row 1 finishes, and the run stops at the boundary after it.
    assert_eq!(indices(&lab, "pr"), vec![0, 1]);
    assert!(!dir.join("run/rows/2").exists());
    assert_eq!(state_of(&lab, "pr"), Some(State::Paused(PAUSED.into())));
    control_of(&lab, "pr").request_resume().unwrap();
    let again = plan_of(&lab, "pr");
    assert_eq!(again.spec_hash, plan.spec_hash);
    assert_eq!(go(&again).unwrap(), RunStatus::Done);
    assert_eq!(indices(&lab, "pr"), vec![0, 1, 2, 3, 4, 5]);
    assert_eq!(state_of(&lab, "pr"), Some(State::Done));
    let ev = events_of(&dir);
    assert_eq!(named(&ev, "run.pause").len(), 1);
    let traces: BTreeSet<&str> = ev.iter().map(|e| e["trace_id"].as_str().unwrap()).collect();
    let trace = hex::encode(&plan.spec_hash[..16]);
    assert_eq!(traces, BTreeSet::from([trace.as_str()]));
}

#[test]
fn a_resume_before_the_boundary_cancels_the_pause() {
    let lab = lab();
    plain(&lab, "cancel");
    let c = control_of(&lab, "cancel");
    c.request_pause().unwrap();
    c.request_resume().unwrap();
    assert_eq!(go(&plan_of(&lab, "cancel")).unwrap(), RunStatus::Done);
    assert_eq!(rows_of(&lab, "cancel").len(), 6);
    assert_eq!(state_of(&lab, "cancel"), Some(State::Done));
}

#[test]
fn stop_cannot_be_resumed() {
    let lab = lab();
    let dir = plain(&lab, "final");
    send_stop(&lab.home, "final", "manual").unwrap();
    let c = control_of(&lab, "final");
    c.request_resume().unwrap();
    c.request_pause().unwrap();
    let stop = Some(Signal::Stop("manual".into()));
    assert_eq!(c.take().unwrap(), stop);
    assert_eq!(c.take().unwrap(), stop);
    assert_eq!(
        state_of(&lab, "final"),
        Some(State::Stopped("manual".into()))
    );
    let plan = plan_of(&lab, "final");
    assert_eq!(go(&plan).unwrap(), RunStatus::Stopped("manual".into()));
    // The signed stop record decides, even when the control file loses its stop.
    std::fs::write(&c.path, "").unwrap();
    assert_eq!(go(&plan).unwrap(), RunStatus::Stopped("manual".into()));
    assert!(rows_of(&lab, "final").is_empty());
    assert!(!dir.join("run/rows").exists());
    assert_eq!(
        state_of(&lab, "final"),
        Some(State::Stopped("manual".into()))
    );
}

#[test]
fn control_signals_keep_the_last_request_and_a_stop_stays() {
    let lab = lab();
    plain(&lab, "sig");
    let c = control_of(&lab, "sig");
    assert_eq!(c.take().unwrap(), None);
    c.request_pause().unwrap();
    assert_eq!(c.peek().unwrap(), Some(Signal::Pause));
    assert_eq!(c.take().unwrap(), Some(Signal::Pause));
    assert_eq!(c.take().unwrap(), None);
    c.request_pause().unwrap();
    c.request_resume().unwrap();
    assert_eq!(c.take().unwrap(), Some(Signal::Resume));
    c.request_stop("one").unwrap();
    c.request_stop("two").unwrap();
    c.request_resume().unwrap();
    assert_eq!(
        std::fs::read_to_string(&c.path).unwrap(),
        "{\"stop\":\"one\"}\n"
    );
    assert_eq!(c.take().unwrap(), Some(Signal::Stop("one".into())));
    // Bytes that do not parse read as a pause, the safe default; the take clears them.
    let other = ControlFile {
        path: lab.root.join("elsewhere/control"),
    };
    std::fs::create_dir(lab.root.join("elsewhere")).unwrap();
    std::fs::write(&other.path, "not a signal\n").unwrap();
    assert_eq!(other.take().unwrap(), Some(Signal::Pause));
    assert_eq!(other.take().unwrap(), None);
    // A study that does not exist gets no folder.
    let ghost = control_of(&lab, "ghost");
    assert!(ghost.request_stop("x").is_err());
    assert!(!lab.home.study_dir("ghost").exists());
    assert!(RunLock::try_take(&lab.home, "ghost").is_err());
}

fn confirmatory(t: String) -> String {
    let ids = |p: &str, n: usize| {
        let all: Vec<String> = (0..n).map(|i| format!("\"{p}{i:02}\"")).collect();
        format!("clusters = [{}]", all.join(", "))
    };
    t.replace("mode = \"exploratory\"", "mode = \"confirmatory\"")
        .replace("label = \"signal\"", "label = \"confirmed\"")
        .replace("sesoi = 0.0", "sesoi = 1.0")
        .replace("value = 0.0", "value = 4.0")
        .replace(
            "source = \"stated\"",
            &format!("source = \"record:{}\"", "a".repeat(64)),
        )
        .replace("units_per_cluster = 3", "units_per_cluster = 5")
        .replacen("clusters = []", &ids("s", 3), 1)
        .replacen("clusters = []", &ids("c", 11), 1)
        .replace(
            "triggers = []",
            "triggers = [{ id = \"T1\", metric = \"ppl-ratio\", threshold_e = 1.0 }]",
        )
}

#[test]
fn stop_lowers_the_label_to_signal() {
    let lab = lab();
    register_study(&lab, "conf", confirmatory, &[], PAYLOAD);
    let rec = send_stop(&lab.home, "conf", "manual").unwrap();
    assert_eq!(rec.body["label_before"], "confirmed");
    assert_eq!(rec.body["label_after"], "signal");
    assert_eq!(
        deviations_of(&lab, "conf")[0]["impact"],
        "0 rows ran before the stop; the earnable label drops from confirmed to signal"
    );
}

#[test]
fn deviation_row_has_the_five_stage_timing() {
    use Timing::*;
    let all = [
        BeforeCollection,
        DuringCollection,
        AfterCollectionBeforeAccess,
        AfterAccessBeforeResults,
        AfterResults,
    ];
    assert_eq!(
        serde_json::to_value(all).unwrap(),
        json!([
            "before-collection",
            "during-collection",
            "after-collection-before-access",
            "after-access-before-results",
            "after-results"
        ])
    );
    assert_eq!(Timing::of_stop(0, 0), BeforeCollection);
    assert_eq!(Timing::of_stop(0, 6), BeforeCollection);
    assert_eq!(Timing::of_stop(1, 6), DuringCollection);
    assert_eq!(Timing::of_stop(5, 6), DuringCollection);
    assert_eq!(Timing::of_stop(6, 6), AfterCollectionBeforeAccess);
    // A stop after the last row is after collection.
    let lab = lab();
    plain(&lab, "after");
    assert_eq!(go(&plan_of(&lab, "after")).unwrap(), RunStatus::Done);
    send_stop(&lab.home, "after", "late").unwrap();
    let devs = deviations_of(&lab, "after");
    assert_eq!(devs[0]["timing"], "after-collection-before-access");
    assert_eq!(
        devs[0]["impact"],
        "6 rows ran before the stop; the earnable label drops from signal to signal"
    );
    // Any deviation row lands as given, after the earlier ones.
    let row = DeviationRow {
        kind: "exclusion".into(),
        reason: "r".into(),
        timing: AfterResults,
        original: "o".into(),
        justification: "j".into(),
        impact: "i".into(),
    };
    control::deviation(&lab.home, "after", &row).unwrap();
    let devs = deviations_of(&lab, "after");
    assert_eq!(devs.len(), 2);
    assert_eq!(
        devs[1],
        json!({"kind": "exclusion", "reason": "r", "timing": "after-results",
               "original": "o", "justification": "j", "impact": "i"})
    );
}

#[test]
fn a_second_run_while_one_holds_the_lock_is_busy() {
    let lab = lab();
    plain(&lab, "busy");
    let plan = plan_of(&lab, "busy");
    let held = RunLock::try_take(&lab.home, "busy").unwrap().expect("free");
    assert!(RunLock::try_take(&lab.home, "busy").unwrap().is_none());
    assert!(matches!(go(&plan), Err(RunError::Busy)), "{:?}", go(&plan));
    assert!(rows_of(&lab, "busy").is_empty());
    assert_eq!(state_of(&lab, "busy"), None);
    drop(held);
    assert_eq!(go(&plan).unwrap(), RunStatus::Done);
}

/// Per row: whether the run lock was free, and the state file.
type Seen = Rc<RefCell<Vec<(bool, Option<State>)>>>;

/// Level 0, after a look at the run lock and the state file from inside the row.
struct Peek {
    home: Home,
    id: String,
    seen: Seen,
}

impl Backend for Peek {
    fn level(&self) -> u8 {
        0
    }
    fn probe(&self) -> Probe {
        Probe::Available
    }
    fn run_row(&self, job: &RowJob, watch: &dyn Watch) -> std::io::Result<RowOutcome> {
        let free = RunLock::try_take(&self.home, &self.id).unwrap().is_some();
        let state = State::read(&self.home, &self.id).unwrap();
        self.seen.borrow_mut().push((free, state));
        Level0.run_row(job, watch)
    }
}

#[test]
fn the_run_holds_the_lock_and_marks_its_state() {
    let lab = lab();
    plain(&lab, "peek");
    let mut plan = plan_of(&lab, "peek");
    let seen = Rc::new(RefCell::new(Vec::new()));
    plan.backend = Box::new(Peek {
        home: lab.home.clone(),
        id: "peek".into(),
        seen: seen.clone(),
    });
    assert_eq!(go(&plan).unwrap(), RunStatus::Done);
    assert_eq!(*seen.borrow(), vec![(false, Some(State::Running)); 6]);
    assert_eq!(state_of(&lab, "peek"), Some(State::Done));
    assert!(RunLock::try_take(&lab.home, "peek").unwrap().is_some());
}

/// Each attempt asks for a stop, ticks, and fails as the box would.
struct StopThenFail {
    control: ControlFile,
    calls: Rc<Cell<u32>>,
}

impl Backend for StopThenFail {
    fn level(&self) -> u8 {
        0
    }
    fn probe(&self) -> Probe {
        Probe::Available
    }
    fn run_row(&self, _: &RowJob, watch: &dyn Watch) -> std::io::Result<RowOutcome> {
        self.calls.set(self.calls.get() + 1);
        self.control.request_stop("mid-attempt").unwrap();
        assert!(
            !watch.tick(),
            "the watch must say stop once a stop is pending"
        );
        Ok(RowOutcome {
            exit: Exit::Infra("box fault".into()),
            cpu_seconds: 0.0,
            wall_seconds: 0.0,
            output_hash: String::new(),
            selftest_ok: false,
        })
    }
}

#[test]
fn a_stop_during_a_failed_attempt_is_not_retried() {
    let lab = lab();
    plain(&lab, "noretry");
    let mut plan = plan_of(&lab, "noretry");
    let calls = Rc::new(Cell::new(0));
    plan.backend = Box::new(StopThenFail {
        control: control_of(&lab, "noretry"),
        calls: calls.clone(),
    });
    assert_eq!(go(&plan).unwrap(), RunStatus::Stopped("mid-attempt".into()));
    assert_eq!(calls.get(), 1);
    assert!(rows_of(&lab, "noretry").is_empty());
    assert_eq!(kind_of(&lab, "noretry", "stop").len(), 1);
}

/// The last row sends a stop and never ticks: the stop lands after the run's last look.
struct LateStop {
    control: ControlFile,
}

impl Backend for LateStop {
    fn level(&self) -> u8 {
        0
    }
    fn probe(&self) -> Probe {
        Probe::Available
    }
    fn run_row(&self, job: &RowJob, _: &dyn Watch) -> std::io::Result<RowOutcome> {
        if job
            .env
            .iter()
            .any(|(k, v)| k == "IGNORARIUM_ROW" && v == "5")
        {
            self.control.request_stop("late").unwrap();
        }
        Level0.run_row(job, &|| true)
    }
}

#[test]
fn a_stop_sent_after_the_last_look_is_finished_as_the_run_ends() {
    let lab = lab();
    plain(&lab, "last");
    let mut plan = plan_of(&lab, "last");
    plan.backend = Box::new(LateStop {
        control: control_of(&lab, "last"),
    });
    assert_eq!(go(&plan).unwrap(), RunStatus::Stopped("late".into()));
    assert_eq!(rows_of(&lab, "last").len(), 6);
    let stops = kind_of(&lab, "last", "stop");
    assert_eq!(stops.len(), 1);
    assert_eq!(stops[0].body["rows_done"], 6);
    assert_eq!(state_of(&lab, "last"), Some(State::Stopped("late".into())));
}

#[test]
fn who_is_the_passwd_name_for_the_uid_else_the_number() {
    let passwd = "root:x:0:0:root:/root:/bin/sh\nrunner:x:1001:1001::/srv/runner:/bin/sh\nbroken\n";
    assert_eq!(control::user_of(passwd, 1001), "runner");
    assert_eq!(control::user_of(passwd, 0), "root");
    assert_eq!(control::user_of(passwd, 1000), "1000");
    assert_eq!(control::user_of("", 7), "7");
    assert_eq!(control::who(), os_user());
}

#[test]
fn a_lock_let_go_within_the_grace_is_taken_and_a_held_one_is_not() {
    let lab = lab();
    plain(&lab, "grace");
    let hold_for = |ms: u64| {
        let lock = RunLock::try_take(&lab.home, "grace").unwrap().unwrap();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(ms));
            drop(lock);
        })
    };
    // A holder that lets go after 10 ms, as a child between fork and exec does.
    let brief = hold_for(10);
    assert!(RunLock::try_take(&lab.home, "grace").unwrap().is_some());
    brief.join().unwrap();
    // A live run holds on.
    let long = hold_for(300);
    let t = Instant::now();
    assert!(RunLock::try_take(&lab.home, "grace").unwrap().is_none());
    assert!(
        t.elapsed() < Duration::from_millis(250),
        "{:?}",
        t.elapsed()
    );
    long.join().unwrap();
}
