mod common;

use common::{
    Lab, PAYLOAD, events_of, indices, kind_of, lab, lines_of, named, plain, register_study,
    rows_of, with_payload,
};
use ignorarium::backend::level0::Level0;
use ignorarium::backend::{Backend, Exit, Probe, RowJob, RowOutcome, Watch};
use ignorarium::control::{
    self, ControlError, ControlFile, DeviationRow, RunLock, Signal, State, Timing,
};
use ignorarium::home::Home;
use ignorarium::keys;
use ignorarium::records::Record;
use ignorarium::runner::{self, HEARTBEAT, RunError, RunPlan, RunStatus, prepare_with};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Barrier};
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

/// Row `i` marks itself started, leaves a child that writes `late` after 1.5 s, and sleeps
/// 30 s. A rerun of the row (its old output kept as `out.crashed-0`) finishes at once.
fn slow_row(i: u64) -> String {
    format!(
        r#"#!/bin/sh
if [ "$IGNORARIUM_ROW" = {i} ] && [ ! -e "$IGNORARIUM_OUT/../out.crashed-0" ]; then
  (sleep 1.5; echo late > "$IGNORARIUM_OUT/late") &
  echo started > "$IGNORARIUM_OUT/started"
  sleep 30
fi
echo ok > "$IGNORARIUM_OUT/result"
"#
    )
}

fn plan_of(lab: &Lab, id: &str) -> RunPlan {
    prepare_with(&lab.home, id, vec![Box::new(Level0)]).unwrap()
}

fn control_of(lab: &Lab, id: &str) -> ControlFile {
    ControlFile::of(&lab.home, id)
}

fn go(plan: &RunPlan) -> Result<RunStatus, RunError> {
    runner::run(plan)
}

fn try_stop(home: &Home, id: &str, why: &str) -> Result<Option<Record>, ControlError> {
    let key = keys::load(home).unwrap().unwrap();
    ControlFile::of(home, id).request_stop(why).unwrap();
    control::stop_if_idle(home, id, &key)
}

/// A stop sent the way the `stop` command sends it: the request, then the trace when no run
/// holds the lock.
fn send_stop(home: &Home, id: &str, why: &str) -> Option<Record> {
    try_stop(home, id, why).unwrap()
}

/// `/proc/<pid>/stat` state letter, or `None` once the process is reaped.
fn proc_state(pid: u64) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.trim_start().chars().next()
}

/// The row file a box keeps while row `row` runs, as a dead box leaves it.
fn leave_row_file(lab: &Lab, id: &str, row: u64, pid: u32, start: u64) {
    let run = lab.home.study_dir(id).join("run");
    std::fs::create_dir_all(&run).unwrap();
    let file = json!({"row": row, "span": "0123456789abcdef", "pid": pid, "pgid": pid,
                      "start": start});
    std::fs::write(run.join("row.json"), file.to_string()).unwrap();
}

/// Wait until the box's row file names row `row`, then return it.
fn wait_for_row_file(dir: &Path, row: u64) -> Value {
    let t = Instant::now();
    loop {
        let file = std::fs::read(dir.join("run/row.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        if let Some(f) = file.filter(|f| f["row"] == row)
            && dir.join(format!("run/rows/{row}/out/started")).exists()
        {
            return f;
        }
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "row {row} never started"
        );
        thread::sleep(Duration::from_millis(5));
    }
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

/// Whether the watch says stop within 200 ticks. A tick looks without waiting, so it sees
/// nothing while another handle holds the control file's lock.
fn says_stop(watch: &dyn Watch) -> bool {
    (0..200).any(|_| {
        let stop = !watch.tick();
        if !stop {
            thread::sleep(Duration::from_millis(1));
        }
        stop
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
    let dir = with_payload(&lab, "mid", &slow_row(2));
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
    assert_eq!(stops[0].body["in_flight"], 2);
    assert_eq!(stops[0].body["note"], Value::Null);
    // The killed row's spend lands in one signed spend record.
    let spend = kind_of(&lab, "mid", "spend");
    assert_eq!(spend.len(), 1);
    assert_eq!(spend[0].row, Some(2));
    assert_eq!(spend[0].body["reason"], "stopped: manual");
    let wall = spend[0].body["wall_seconds"].as_f64().unwrap();
    assert!((0.0..10.0).contains(&wall) && wall > 0.0, "{wall}");
    assert!(spend[0].body["cpu_seconds"].as_f64().unwrap() >= 0.0);
    assert!(!dir.join("run/row.json").exists());
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
               "label_after": "signal", "rows_done": 0, "in_flight": null, "note": null})
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
    let again = lock.stop(&key, "third", Some(4)).unwrap();
    assert_eq!(again, first);
    drop(lock);
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
    let dir = with_payload(&lab, "race", &slow_row(2));
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
    let of =
        |done, in_flight, total: usize| Timing::of_stop(done, in_flight, || Ok(total)).unwrap();
    assert_eq!(of(0, false, 0), BeforeCollection);
    assert_eq!(of(0, false, 6), BeforeCollection);
    assert_eq!(of(1, false, 6), DuringCollection);
    assert_eq!(of(5, false, 6), DuringCollection);
    assert_eq!(of(6, false, 6), AfterCollectionBeforeAccess);
    // A row killed in flight was collecting, even the first.
    assert_eq!(of(0, true, 6), DuringCollection);
    assert_eq!(of(5, true, 6), DuringCollection);
    // The schedule is read only when it decides; a missing one is then an error.
    let missing = || Err(std::io::Error::other("no schedule.json"));
    assert_eq!(
        Timing::of_stop(0, false, missing).unwrap(),
        BeforeCollection
    );
    assert_eq!(Timing::of_stop(3, true, missing).unwrap(), DuringCollection);
    assert!(Timing::of_stop(3, false, missing).is_err());
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
            says_stop(watch),
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
    // The holder thread runs before the clock starts, so only its hold counts.
    let hold_for = |ms: u64| {
        let lock = RunLock::try_take(&lab.home, "grace").unwrap().unwrap();
        let gate = Arc::new(Barrier::new(2));
        let at_gate = gate.clone();
        let holder = thread::spawn(move || {
            at_gate.wait();
            thread::sleep(Duration::from_millis(ms));
            drop(lock);
        });
        gate.wait();
        holder
    };
    // A holder that lets go after 2 ms, as a child between fork and exec does.
    let brief = hold_for(2);
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

/// Sends a stop without ticking, then breaks the event log, so the run fails after the row.
struct StopThenBreakEvents {
    control: ControlFile,
    dir: std::path::PathBuf,
}

impl Backend for StopThenBreakEvents {
    fn level(&self) -> u8 {
        0
    }
    fn probe(&self) -> Probe {
        Probe::Available
    }
    fn run_row(&self, job: &RowJob, _: &dyn Watch) -> std::io::Result<RowOutcome> {
        self.control.request_stop("broken").unwrap();
        std::fs::rename(self.dir.join("events.jsonl"), self.dir.join("events.old")).unwrap();
        std::fs::create_dir(self.dir.join("events.jsonl")).unwrap();
        Level0.run_row(job, &|| true)
    }
}

#[test]
fn a_run_that_errors_after_a_stop_still_leaves_the_stop_trace() {
    let lab = lab();
    let dir = plain(&lab, "err-stop");
    let mut plan = plan_of(&lab, "err-stop");
    plan.backend = Box::new(StopThenBreakEvents {
        control: control_of(&lab, "err-stop"),
        dir: dir.clone(),
    });
    let err = go(&plan).expect_err("the row.end write fails");
    assert!(matches!(err, RunError::Io(_)), "{err}");
    assert_eq!(indices(&lab, "err-stop"), vec![0]);
    let stops = kind_of(&lab, "err-stop", "stop");
    assert_eq!(stops.len(), 1);
    assert_eq!(stops[0].body["why"], "broken");
    assert_eq!(deviations_of(&lab, "err-stop").len(), 1);
    assert_eq!(
        state_of(&lab, "err-stop"),
        Some(State::Stopped("broken".into()))
    );
}

/// Runs only as the child of the dead-box tests: the box that gets killed mid-row.
#[test]
fn dead_box_child() {
    let (Some(root), Some(id)) = (
        std::env::var_os("CONTROL_BOX_HOME"),
        std::env::var("CONTROL_BOX_STUDY").ok(),
    ) else {
        return;
    };
    let home = Home::open_at(Path::new(&root)).unwrap();
    let plan = prepare_with(&home, &id, vec![Box::new(Level0)]).unwrap();
    let _ = runner::run(&plan);
}

/// Run study `id` in a child box, kill the box once row `row` runs, and return its row file.
fn kill_the_box_at(lab: &Lab, id: &str, row: u64) -> Value {
    let mut boxed = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "dead_box_child", "--nocapture"])
        .env("CONTROL_BOX_HOME", &lab.home.root)
        .env("CONTROL_BOX_STUDY", id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let file = wait_for_row_file(&lab.home.study_dir(id), row);
    boxed.kill().unwrap();
    boxed.wait().unwrap();
    // The row's group outlives its box.
    let pid = file["pid"].as_u64().unwrap();
    assert!(
        matches!(proc_state(pid), Some(s) if s != 'Z'),
        "{:?}",
        proc_state(pid)
    );
    file
}

#[test]
fn a_stop_after_the_box_dies_kills_the_row_left_running() {
    let lab = lab();
    let dir = with_payload(&lab, "dead", &slow_row(2));
    let file = kill_the_box_at(&lab, "dead", 2);
    let rec = send_stop(&lab.home, "dead", "box died").expect("no box holds the lock");
    assert_eq!(rec.body["in_flight"], 2);
    assert_eq!(rec.body["note"], "row leader alive; group killed");
    assert_eq!(rec.body["rows_done"], 2);
    assert_eq!(
        deviations_of(&lab, "dead")[0]["timing"],
        "during-collection"
    );
    assert!(!dir.join("run/row.json").exists());
    thread::sleep(Duration::from_millis(2000));
    assert!(!dir.join("run/rows/2/out/late").exists());
    let pid = file["pid"].as_u64().unwrap();
    assert!(
        matches!(proc_state(pid), None | Some('Z')),
        "{:?}",
        proc_state(pid)
    );
}

#[test]
fn the_next_run_kills_a_row_a_dead_box_left_running() {
    let lab = lab();
    let dir = with_payload(&lab, "dead-run", &slow_row(2));
    let file = kill_the_box_at(&lab, "dead-run", 2);
    assert_eq!(go(&plan_of(&lab, "dead-run")).unwrap(), RunStatus::Done);
    assert_eq!(indices(&lab, "dead-run"), vec![0, 1, 2, 3, 4, 5]);
    let pid = file["pid"].as_u64().unwrap();
    assert!(
        matches!(proc_state(pid), None | Some('Z')),
        "{:?}",
        proc_state(pid)
    );
    // The dead box's row span gets its end.
    let ev = events_of(&dir);
    let ended: Vec<_> = named(&ev, "row.end")
        .into_iter()
        .filter(|e| e["span_id"] == file["span"])
        .collect();
    assert_eq!(ended.len(), 1);
    assert_eq!(
        ended[0]["attrs"],
        json!({"row": 2, "exit": "orphaned", "note": "row leader alive; group killed"})
    );
    thread::sleep(Duration::from_millis(2000));
    assert!(!dir.join("run/rows/2/out.crashed-0/late").exists());
}

#[test]
fn a_left_row_is_killed_only_while_its_leader_lives() {
    let lab = lab();
    // A live group whose leader has another start time is not the row's: it lives.
    plain(&lab, "other-start");
    let mut live = Command::new("sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    leave_row_file(&lab, "other-start", 1, live.id(), 1);
    let rec = send_stop(&lab.home, "other-start", "x").unwrap();
    assert_eq!(rec.body["note"], "row leader gone; group not killed");
    assert_eq!(rec.body["in_flight"], 1);
    assert!(
        live.try_wait().unwrap().is_none(),
        "another group was killed"
    );
    live.kill().unwrap();
    live.wait().unwrap();
    // A leader already reaped: nothing to kill.
    plain(&lab, "reaped");
    let mut gone = Command::new("true").spawn().unwrap();
    let pid = gone.id();
    gone.wait().unwrap();
    leave_row_file(&lab, "reaped", 0, pid, 1);
    let rec = send_stop(&lab.home, "reaped", "x").unwrap();
    assert_eq!(rec.body["note"], "row leader gone; group not killed");
    assert_eq!(rec.body["in_flight"], 0);
    assert_eq!(
        deviations_of(&lab, "reaped")[0]["timing"],
        "during-collection"
    );
}

/// Makes the row file unwritable inside the row, before the backend starts it.
struct NoRowFile {
    dir: std::path::PathBuf,
}

impl Backend for NoRowFile {
    fn level(&self) -> u8 {
        0
    }
    fn probe(&self) -> Probe {
        Probe::Available
    }
    fn run_row(&self, job: &RowJob, watch: &dyn Watch) -> std::io::Result<RowOutcome> {
        std::fs::create_dir_all(self.dir.join("run/row.json")).unwrap();
        Level0.run_row(job, watch)
    }
}

#[test]
fn a_row_the_box_cannot_record_is_stopped_and_the_run_pauses() {
    let lab = lab();
    let dir = with_payload(&lab, "untracked", &slow_row(0));
    let mut plan = plan_of(&lab, "untracked");
    plan.backend = Box::new(NoRowFile { dir: dir.clone() });
    let t = Instant::now();
    let status = go(&plan).unwrap();
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
    let why = match &status {
        RunStatus::Paused(m) if m.starts_with("cannot record the running row: ") => m.clone(),
        s => panic!("{s:?}"),
    };
    assert!(rows_of(&lab, "untracked").is_empty());
    let spend = kind_of(&lab, "untracked", "spend");
    assert_eq!(spend.len(), 1);
    assert_eq!(spend[0].row, Some(0));
    assert_eq!(spend[0].body["reason"], why.as_str());
    // A row file that cannot be read stops the next run before any row.
    let status = go(&plan_of(&lab, "untracked")).unwrap();
    assert!(
        matches!(&status, RunStatus::Paused(m) if m.starts_with("cannot end the row a dead box left: ")),
        "{status:?}"
    );
    assert!(rows_of(&lab, "untracked").is_empty());
    // A stop still works, and its record says the left row could not be handled.
    let rec = send_stop(&lab.home, "untracked", "manual").unwrap();
    let note = rec.body["note"].as_str().unwrap();
    assert!(
        note.starts_with("cannot end the row a dead box left: ")
            && note.ends_with("; group not killed"),
        "{note}"
    );
    assert_eq!(rec.body["in_flight"], Value::Null);
}

#[test]
fn an_unreadable_control_file_pauses_the_run() {
    let lab = lab();
    let dir = plain(&lab, "blind");
    let plan = plan_of(&lab, "blind");
    std::fs::create_dir(dir.join("run")).unwrap();
    std::os::unix::fs::symlink(lab.root.join("elsewhere"), dir.join("run/control")).unwrap();
    let status = go(&plan).unwrap();
    let why = match &status {
        RunStatus::Paused(m) if m.starts_with("the control file cannot be read: ") => m.clone(),
        s => panic!("{s:?}"),
    };
    assert!(rows_of(&lab, "blind").is_empty());
    let ev = events_of(&dir);
    assert_eq!(named(&ev, "run.pause").len(), 1);
    assert_eq!(state_of(&lab, "blind"), Some(State::Paused(why)));
}

/// Sends a stop, sees it on a tick, then makes the control file unreadable.
struct StopThenBlind {
    control: ControlFile,
}

impl Backend for StopThenBlind {
    fn level(&self) -> u8 {
        0
    }
    fn probe(&self) -> Probe {
        Probe::Available
    }
    fn run_row(&self, _: &RowJob, watch: &dyn Watch) -> std::io::Result<RowOutcome> {
        self.control.request_stop("seen").unwrap();
        assert!(says_stop(watch));
        let path = &self.control.path;
        std::fs::rename(path, path.with_file_name("control.old")).unwrap();
        std::os::unix::fs::symlink("/nonexistent", path).unwrap();
        Ok(RowOutcome {
            exit: Exit::Ok,
            cpu_seconds: 0.0,
            wall_seconds: 0.0,
            output_hash: String::new(),
            selftest_ok: true,
        })
    }
}

#[test]
fn a_stop_seen_mid_row_wins_over_a_later_read_error() {
    let lab = lab();
    plain(&lab, "seen");
    let mut plan = plan_of(&lab, "seen");
    plan.backend = Box::new(StopThenBlind {
        control: control_of(&lab, "seen"),
    });
    assert_eq!(go(&plan).unwrap(), RunStatus::Stopped("seen".into()));
    let stops = kind_of(&lab, "seen", "stop");
    assert_eq!(stops.len(), 1);
    assert_eq!(stops[0].body["in_flight"], 0);
    assert!(rows_of(&lab, "seen").is_empty());
}

#[test]
fn the_tick_never_waits_for_the_control_lock() {
    let lab = lab();
    let dir = with_payload(&lab, "ctl-busy", SLOW_ROW_1);
    let mut plan = plan_of(&lab, "ctl-busy");
    plan.heartbeat = Duration::from_millis(50);
    let path = control_of(&lab, "ctl-busy").path;
    // Another handle holds the control file's lock for 1.5 s while row 1 sleeps 0.5 s.
    let holder = once_started(&lab.home, "ctl-busy", 1, move |_| {
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        f.lock().unwrap();
        thread::sleep(Duration::from_millis(1500));
    });
    assert_eq!(go(&plan).unwrap(), RunStatus::Done);
    holder.join().unwrap();
    let wall = rows_of(&lab, "ctl-busy")[1].body["wall_seconds"]
        .as_f64()
        .unwrap();
    assert!(wall < 1.2, "row 1 waited for the control lock: {wall} s");
    let ev = events_of(&dir);
    let beats = named(&ev, "heartbeat")
        .into_iter()
        .filter(|e| e["attrs"]["row"] == 1)
        .count();
    assert!(beats >= 4, "{beats}");
}

/// Sends a pause, ticks, and notes what the control file holds after the tick.
struct PauseThenLook {
    control: ControlFile,
    seen: Rc<RefCell<Vec<Option<Signal>>>>,
}

impl Backend for PauseThenLook {
    fn level(&self) -> u8 {
        0
    }
    fn probe(&self) -> Probe {
        Probe::Available
    }
    fn run_row(&self, job: &RowJob, watch: &dyn Watch) -> std::io::Result<RowOutcome> {
        self.control.request_pause().unwrap();
        assert!(watch.tick(), "a pause never stops a row");
        self.seen.borrow_mut().push(self.control.peek().unwrap());
        Level0.run_row(job, watch)
    }
}

#[test]
fn a_pause_sent_mid_row_stays_in_the_file_until_the_boundary() {
    let lab = lab();
    plain(&lab, "keep");
    let mut plan = plan_of(&lab, "keep");
    let seen = Rc::new(RefCell::new(Vec::new()));
    plan.backend = Box::new(PauseThenLook {
        control: control_of(&lab, "keep"),
        seen: seen.clone(),
    });
    assert_eq!(go(&plan).unwrap(), RunStatus::Paused(PAUSED.into()));
    assert_eq!(*seen.borrow(), vec![Some(Signal::Pause)]);
    assert_eq!(indices(&lab, "keep"), vec![0]);
    assert_eq!(control_of(&lab, "keep").peek().unwrap(), None);
}

#[test]
fn a_later_stop_fills_in_what_an_earlier_one_could_not_write() {
    let lab = lab();
    let dir = plain(&lab, "repair");
    assert_eq!(go(&plan_of(&lab, "repair")).unwrap(), RunStatus::Done);
    // A corrupt schedule: the stop record and the state land, the deviation row cannot.
    let schedule = std::fs::read(dir.join("schedule.json")).unwrap();
    std::fs::write(dir.join("schedule.json"), "{").unwrap();
    let err = try_stop(&lab.home, "repair", "late").unwrap_err();
    assert!(err.to_string().contains("schedule.json"), "{err}");
    assert_eq!(kind_of(&lab, "repair", "stop").len(), 1);
    assert_eq!(
        state_of(&lab, "repair"),
        Some(State::Stopped("late".into()))
    );
    assert!(deviations_of(&lab, "repair").is_empty());
    assert!(named(&events_of(&dir), "run.stop").is_empty());
    // With the schedule back, the next call writes only what is missing.
    std::fs::write(dir.join("schedule.json"), &schedule).unwrap();
    let rec = send_stop(&lab.home, "repair", "again").unwrap();
    assert_eq!(rec.body["why"], "late");
    let devs = deviations_of(&lab, "repair");
    assert_eq!(devs.len(), 1);
    assert_eq!(devs[0]["timing"], "after-collection-before-access");
    assert_eq!(named(&events_of(&dir), "run.stop").len(), 1);
    assert_eq!(kind_of(&lab, "repair", "stop").len(), 1);
    // A lost run.stop event is written again, once.
    let kept: String = std::fs::read_to_string(dir.join("events.jsonl"))
        .unwrap()
        .lines()
        .filter(|l| !l.contains("\"run.stop\""))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::rename(dir.join("events.jsonl"), dir.join("events.old")).unwrap();
    std::fs::write(dir.join("events.jsonl"), kept).unwrap();
    send_stop(&lab.home, "repair", "third").unwrap();
    send_stop(&lab.home, "repair", "fourth").unwrap();
    assert_eq!(named(&events_of(&dir), "run.stop").len(), 1);
    assert_eq!(deviations_of(&lab, "repair").len(), 1);
}

#[test]
fn a_stop_that_kills_row_0_is_during_collection() {
    let lab = lab();
    let dir = with_payload(&lab, "first0", &slow_row(0));
    let plan = plan_of(&lab, "first0");
    let other = once_started(&lab.home, "first0", 0, |home| {
        send_stop(home, "first0", "manual");
    });
    assert_eq!(go(&plan).unwrap(), RunStatus::Stopped("manual".into()));
    other.join().unwrap();
    let stop = &kind_of(&lab, "first0", "stop")[0];
    assert_eq!(stop.body["rows_done"], 0);
    assert_eq!(stop.body["in_flight"], 0);
    assert_eq!(
        deviations_of(&lab, "first0")[0]["timing"],
        "during-collection"
    );
    assert_eq!(kind_of(&lab, "first0", "spend").len(), 1);
    assert!(!dir.join("run/rows/1").exists());
}

#[test]
fn reads_create_no_run_folder() {
    let lab = lab();
    let dir = plain(&lab, "untouched");
    let key = keys::load(&lab.home).unwrap().unwrap();
    assert_eq!(state_of(&lab, "untouched"), None);
    assert_eq!(control_of(&lab, "untouched").peek().unwrap(), None);
    assert_eq!(control_of(&lab, "untouched").take().unwrap(), None);
    assert!(
        control::stop_if_idle(&lab.home, "untouched", &key)
            .unwrap()
            .is_none()
    );
    assert!(!dir.join("run").exists());
    // Once run/ exists, a read still adds no control file.
    State::Done.write(&lab.home, "untouched").unwrap();
    assert_eq!(control_of(&lab, "untouched").peek().unwrap(), None);
    assert_eq!(control_of(&lab, "untouched").take().unwrap(), None);
    assert!(!dir.join("run/control").exists());
}

#[test]
fn reads_refuse_a_linked_or_broken_run_folder() {
    let lab = lab();
    // A linked run folder is refused, even when its target holds a state.
    let dir = plain(&lab, "linked");
    let elsewhere = lab.root.join("elsewhere-run");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("state.json"), "\"done\"\n").unwrap();
    std::os::unix::fs::symlink(&elsewhere, dir.join("run")).unwrap();
    assert!(State::read(&lab.home, "linked").is_err());
    assert!(control_of(&lab, "linked").peek().is_err());
    assert!(control::run_is_live(&lab.home, "linked").is_err());
    // Under a study folder that is a file, no request can be read.
    std::fs::write(lab.home.study_dir("flat"), "x").unwrap();
    assert!(control_of(&lab, "flat").peek().is_err());
    // A state that is a folder is an error, not a study that never ran.
    let dir = plain(&lab, "odd");
    std::fs::create_dir_all(dir.join("run/state.json")).unwrap();
    assert!(State::read(&lab.home, "odd").is_err());
    // A lock that cannot be opened is an error, not a free lock.
    std::os::unix::fs::symlink("lock", dir.join("run/lock")).unwrap();
    assert!(control::run_is_live(&lab.home, "odd").is_err());
}

#[test]
fn a_stop_never_appends_to_a_deviation_log_it_cannot_read() {
    let lab = lab();
    let dir = plain(&lab, "garbled");
    send_stop(&lab.home, "garbled", "first").unwrap();
    let log = dir.join("deviations.jsonl");
    let mut bytes = std::fs::read(&log).unwrap();
    bytes.extend_from_slice(b"\xff\n");
    std::fs::write(&log, &bytes).unwrap();
    assert!(try_stop(&lab.home, "garbled", "again").is_err());
    assert_eq!(std::fs::read(&log).unwrap(), bytes);
}

#[test]
fn errors_read_as_plain_text() {
    assert_eq!(
        RunError::Busy.to_string(),
        "another run of this study is in progress"
    );
    assert_eq!(
        ControlError::Records("bad chain".into()).to_string(),
        "records: bad chain"
    );
    assert_eq!(
        ControlError::Io(std::io::Error::other("gone")).to_string(),
        "I/O failed: gone"
    );
}

#[test]
fn two_threads_stopping_an_idle_study_at_once_write_one_record() {
    let lab = lab();
    let dir = plain(&lab, "both");
    let gate = Arc::new(Barrier::new(2));
    let senders = ["one", "two"].map(|why| {
        let (home, gate) = (lab.home.clone(), gate.clone());
        thread::spawn(move || {
            gate.wait();
            send_stop(&home, "both", why)
        })
    });
    let got: Vec<Option<Record>> = senders.into_iter().map(|h| h.join().unwrap()).collect();
    let stops = kind_of(&lab, "both", "stop");
    assert_eq!(stops.len(), 1);
    assert!(got.iter().any(Option::is_some), "{got:?}");
    assert!(got.iter().flatten().all(|r| *r == stops[0]));
    assert_eq!(deviations_of(&lab, "both").len(), 1);
    assert_eq!(named(&events_of(&dir), "run.stop").len(), 1);
    let why = stops[0].body["why"].as_str().unwrap().to_string();
    assert_eq!(state_of(&lab, "both"), Some(State::Stopped(why)));
}

#[test]
fn await_stop_returns_the_record_an_idle_study_gets() {
    let lab = lab();
    plain(&lab, "aw1");
    let key = keys::load(&lab.home).unwrap().unwrap();
    ControlFile::of(&lab.home, "aw1")
        .request_stop("enough")
        .unwrap();
    let r = control::await_stop(&lab.home, "aw1", &key, Duration::from_millis(100))
        .unwrap()
        .expect("an idle study records the stop at once");
    assert_eq!(r.kind, "stop");
    assert_eq!(r.body["why"], "enough");
    assert_eq!(kind_of(&lab, "aw1", "stop").len(), 1);
}

#[test]
fn await_stop_times_out_while_a_live_run_never_answers() {
    let lab = lab();
    plain(&lab, "aw2");
    let key = keys::load(&lab.home).unwrap().unwrap();
    let held = RunLock::try_take(&lab.home, "aw2").unwrap().unwrap();
    ControlFile::of(&lab.home, "aw2")
        .request_stop("later")
        .unwrap();
    let start = Instant::now();
    let got = control::await_stop(&lab.home, "aw2", &key, Duration::from_millis(150)).unwrap();
    assert!(got.is_none());
    assert!(start.elapsed() >= Duration::from_millis(150));
    assert!(kind_of(&lab, "aw2", "stop").is_empty());
    drop(held);
}

#[test]
fn await_stop_returns_a_record_the_live_run_writes_while_it_waits() {
    let lab = lab();
    plain(&lab, "aw3");
    let key = keys::load(&lab.home).unwrap().unwrap();
    let held = RunLock::try_take(&lab.home, "aw3").unwrap().unwrap();
    ControlFile::of(&lab.home, "aw3")
        .request_stop("live")
        .unwrap();
    let (home, k) = (lab.home.clone(), key.clone());
    let waiter = thread::spawn(move || {
        control::await_stop(&home, "aw3", &k, Duration::from_secs(10)).unwrap()
    });
    thread::sleep(Duration::from_millis(200));
    held.stop(&key, "live", None).unwrap();
    let r = waiter.join().unwrap().expect("the record the run wrote");
    assert_eq!(r.kind, "stop");
}

#[test]
fn run_is_live_probes_without_taking_the_lock() {
    let lab = lab();
    plain(&lab, "lv");
    assert!(!control::run_is_live(&lab.home, "lv").unwrap());
    assert!(
        !lab.home.study_dir("lv").join("run").exists(),
        "reads make no run/"
    );
    let held = RunLock::try_take(&lab.home, "lv").unwrap().unwrap();
    assert!(control::run_is_live(&lab.home, "lv").unwrap());
    assert!(
        control::run_is_live(&lab.home, "lv").unwrap(),
        "a probe leaves it held"
    );
    drop(held);
    assert!(!control::run_is_live(&lab.home, "lv").unwrap());
    // A probe in progress does not make a run Busy: the lock is free once it returns.
    assert!(RunLock::try_take(&lab.home, "lv").unwrap().is_some());
}
