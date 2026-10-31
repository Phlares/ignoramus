mod common;

use common::{
    Lab, PAYLOAD, events_of, indices, kind_of, lab, named, plain, register_study, rows_of,
};
use ignorarium::backend::level0::Level0;
use ignorarium::backend::{Backend, Exit, Probe, RowJob, RowOutcome, Watch};
use ignorarium::budget::{Cost, FakeMeter, MeterError};
use ignorarium::canon::sha256_hex;
use ignorarium::control::State;
use ignorarium::derived::{
    Analysis, BoundResult, Estimate, Register, StudyState, Verdict, contrast_key,
};
use ignorarium::planlog::store::PlanLog;
use ignorarium::prior::{PriorUse, file_hashes};
use ignorarium::runner::{RunError, RunPlan, RunStatus, prepare, prepare_with, run, up_to};
use ignorarium::stream::{Log, LogName};
use ignorarium::{analysis, estimate, keys, records, schedule, spec};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

/// The template's confirmation input at `data/conf.txt`.
fn conf_input(t: String) -> String {
    t.replace("data/confirm.txt", "data/conf.txt")
}

/// The template with a selection stage: `arm-a` and `arm-b` (each with params) on the 2 units
/// of the selection input `data/select.txt`, then `selected` on the confirmation input. The
/// schedule: rows 0 to 5 select, rows 6 to 11 confirm. The hypothesis tests the pick.
fn staged(t: String) -> String {
    let stage = r#"id = "arm-a"
params = { bits = 4, mode = "fast" }
[[design.conditions]]
id = "arm-b"
params = { bits = 8 }
[design.selection]
metric = "ppl-ratio"
conditions = ["arm-a", "arm-b"]
rule = "best-mean"
"#;
    let input = r#"[[inputs]]
path = "data/select.txt"
sha256 = ""
role = "selection"
stratum = "fiction"
units = 2
[inputs.licence]
spdx = ""
type = "unspecified"
source = ""
creators = []

[pins]"#;
    t.replacen("id = \"arm-a\"\n", stage, 1)
        .replacen(
            "conditions = [\"arm-a\"]   ",
            "conditions = [\"selected\"]",
            1,
        )
        .replacen("[pins]", input, 1)
        .replacen(
            "contrast = { arm-a = 1.0 }",
            "contrast = { selected = 1.0 }",
            1,
        )
}

/// Writes its params and its argv, and a value per condition: against the reference's 8,
/// `arm-a` pairs to +25 % and `arm-b` to -25 %, so the lower-is-better pick is `arm-b`.
const STAGED: &str = "#!/bin/sh
printf '%s' \"$IGNORARIUM_PARAMS\" > \"$IGNORARIUM_OUT/params\"
printf '%s %s %s' \"$2\" \"$4\" \"$6\" > \"$IGNORARIUM_OUT/result\"
case \"$2\" in exact) v=8;; arm-a) v=10;; arm-b) v=6;; *) v=0;; esac
printf '{\"ppl\": %s}' \"$v\" > \"$IGNORARIUM_OUT/values.json\"
";

fn selections_of(lab: &Lab, id: &str) -> Vec<records::Record> {
    kind_of(lab, id, "selection")
}

#[test]
fn selected_rows_run_the_pick_and_name_the_record() {
    let lab = lab();
    let dir = register_study(&lab, "pick", staged, &[], STAGED);
    let plan = plan_of(&lab, "pick");
    let stages: Vec<_> = plan.schedule.rows.iter().map(|r| &r.stage[..]).collect();
    assert_eq!(stages, [["selection"; 6], ["confirmation"; 6]].concat());
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let sel = selections_of(&lab, "pick");
    assert_eq!(sel.len(), 1);
    assert_eq!(
        sel[0].body,
        serde_json::json!({
            "metric": "ppl-ratio",
            "rule": "best-mean",
            "direction": "lower",
            "candidates": [{ "id": "arm-a", "mean": 25.0 }, { "id": "arm-b", "mean": -25.0 }],
            "selected": "arm-b",
            "units": ["data/select.txt#0", "data/select.txt#1"],
            "complete_units": 2,
            "incomplete_units": 0,
        })
    );
    // Written at the stage boundary: after the last selection row, before any confirmation row.
    let all = common::records_of(&lab, "pick");
    let at = all.iter().position(|r| r.kind == "selection").unwrap();
    assert_eq!((all[at - 1].row, all[at + 1].row), (Some(5), Some(6)));
    let hash = records::hash(&sel[0]);
    for r in rows_of(&lab, "pick") {
        let i = r.row.unwrap();
        let row = &plan.schedule.rows[i as usize];
        assert_eq!(r.body["stage"], row.stage.as_str());
        assert_eq!(r.body["condition"], row.condition.as_str());
        let ran = std::fs::read_to_string(dir.join(format!("run/rows/{i}/out/result"))).unwrap();
        if row.condition == "selected" {
            assert_eq!(r.body["picked"], "arm-b");
            assert_eq!(r.body["selection"], hash.as_str());
            assert_eq!(ran, format!("arm-b {} 0", row.unit));
        } else {
            assert!(r.body.get("picked").is_none() && r.body.get("selection").is_none());
            assert_eq!(ran, format!("{} {} 0", row.condition, row.unit));
        }
    }
}

#[test]
fn params_reach_the_payload_as_json() {
    let lab = lab();
    let dir = register_study(&lab, "params", staged, &[], STAGED);
    let plan = plan_of(&lab, "params");
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    for row in &plan.schedule.rows {
        let got = std::fs::read_to_string(dir.join(format!("run/rows/{}/out/params", row.index)))
            .unwrap();
        // Canonical JSON: sorted keys, no spaces; `{}` for a condition without params.
        let want = match row.condition.as_str() {
            "exact" => "{}",
            "arm-a" => r#"{"bits":4,"mode":"fast"}"#,
            _ => r#"{"bits":8}"#,
        };
        assert_eq!(got, want, "row {}", row.index);
    }
}

#[test]
fn a_resumed_run_reuses_the_selection_record() {
    let lab = lab();
    register_study(&lab, "reuse", staged, &[], STAGED);
    let mut plan = plan_of(&lab, "reuse");
    // The box dies in row 6, the first confirmation row, after the selection record.
    let (mut b, _) = fake(vec![]);
    b.panic_at = Some(6);
    plan.backend = Box::new(b);
    assert!(catch_unwind(AssertUnwindSafe(|| run(&plan))).is_err());
    assert_eq!(indices(&lab, "reuse"), (0..6).collect::<Vec<_>>());
    let first = selections_of(&lab, "reuse");
    assert_eq!(first.len(), 1);
    plan.backend = Box::new(Level0);
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    assert_eq!(selections_of(&lab, "reuse"), first);
    let hash = records::hash(&first[0]);
    let named: Vec<_> = rows_of(&lab, "reuse")
        .into_iter()
        .filter(|r| r.body["condition"] == "selected")
        .map(|r| r.body["selection"].clone())
        .collect();
    assert_eq!(named, vec![serde_json::json!(hash); 3]);
}

#[test]
fn no_complete_unit_pauses_the_run() {
    let lab = lab();
    // The candidates write no values: their rows are void, and no selection unit is complete.
    let payload = STAGED.replace("case", "case \"$2\" in arm-*) exit 0;; esac\ncase");
    register_study(&lab, "nopick", staged, &[], &payload);
    let plan = plan_of(&lab, "nopick");
    let why = "no selection unit has a ppl-ratio value for every candidate; incomplete units: 2";
    assert_eq!(run(&plan).unwrap(), RunStatus::Paused(why.into()));
    // No confirmation row ran, the reference's included.
    assert_eq!(indices(&lab, "nopick"), (0..6).collect::<Vec<_>>());
    assert!(selections_of(&lab, "nopick").is_empty());
    let dir = lab.home.study_dir("nopick");
    let pauses = events_of(&dir);
    assert_eq!(named(&pauses, "run.pause")[0]["attrs"]["reason"], why);
    // A resume pauses again, at the same row.
    assert_eq!(run(&plan).unwrap(), RunStatus::Paused(why.into()));
    assert_eq!(indices(&lab, "nopick").len(), 6);
}

#[test]
fn the_pick_waits_for_every_selection_row() {
    let lab = lab();
    register_study(&lab, "partial", staged, &[], STAGED);
    let mut plan = plan_of(&lab, "partial");
    // Selection row 5 moves past the stage boundary: the pick would miss its unit.
    let row5 = plan.schedule.rows.remove(5);
    plan.schedule.rows.push(row5);
    let why = "selection row 5 has no record; the pick needs every selection row";
    assert_eq!(run(&plan).unwrap(), RunStatus::Paused(why.into()));
    assert_eq!(indices(&lab, "partial"), (0..5).collect::<Vec<_>>());
    assert!(selections_of(&lab, "partial").is_empty());
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
    fn cpus(&self) -> u32 {
        1
    }
    fn launcher(&self) -> String {
        "process".into()
    }
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
    prepare_with(
        &lab.home,
        id,
        &common::work_of(&lab.home, id),
        vec![Box::new(Level0)],
    )
    .unwrap()
}

fn err_of(lab: &Lab, id: &str) -> RunError {
    prepare_with(
        &lab.home,
        id,
        &common::work_of(&lab.home, id),
        vec![Box::new(Level0)],
    )
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
    // Register, inputs, 6 rows, then the analysis, estimate and verdict records.
    assert_eq!(records::verify(&log, &key.verifying_key()).unwrap(), 11);
    let rows = rows_of(&lab, "s1");
    assert_eq!(rows.len(), 6);
    for (i, r) in rows.iter().enumerate() {
        assert_eq!(r.row, Some(i as u64));
        assert_eq!(r.seq, i as u64 + 2);
        let row = &plan.schedule.rows[i];
        assert_eq!(r.body["condition"], row.condition.as_str());
        assert_eq!(r.body["unit"], row.unit.as_str());
        assert_eq!(r.body["repeat"], row.repeat);
        assert_eq!(r.body["seed"], row.seed_hex.as_str());
        assert_eq!(r.body["exit"], "ok");
        assert_eq!(r.body["attempts"], 1);
        assert_eq!(r.body["isolation"], 0);
        assert_eq!(r.body["launcher"], "process");
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
    // Keep the register and inputs records and rows 0..3.
    let log = dir.join("records.jsonl");
    let text = std::fs::read_to_string(&log).unwrap();
    let kept: String = text.split_inclusive('\n').take(5).collect();
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
         echo ok > \"$IGNORARIUM_OUT/result\"\n{}\n",
        common::VALUES
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
    // The copy is read-only; a row at level 0 can still make it writable.
    let edit = edits_in_row_0("chmod u+w \"$0\"; printf '# edited\n' >> \"$0\"");
    common::with_payload(&lab, "edit-pay", &edit);
    pauses_after_row_0(&lab, "edit-pay", "payload changed after the run began");
}

#[test]
fn editing_a_helper_file_pauses_before_the_next_attempt() {
    let lab = lab();
    let edit =
        edits_in_row_0("h=\"${0%/*}/helper.sh\"; chmod u+w \"$h\"; printf '# edited\n' >> \"$h\"");
    let helper = [("payload/helper.sh", "echo helper\n")];
    register_study(&lab, "edit-helper", |t| t, &helper, &edit);
    pauses_after_row_0(&lab, "edit-helper", "payload changed after the run began");
}

#[test]
fn a_file_added_to_the_input_view_mid_run_pauses_before_the_next_row() {
    let lab = lab();
    let edit =
        edits_in_row_0("chmod u+w \"$IGNORARIUM_IN\"; printf x > \"$IGNORARIUM_IN/extra.txt\"");
    common::with_payload(&lab, "add-input", &edit);
    pauses_after_row_0(
        &lab,
        "add-input",
        "the input view changed after the run began",
    );
}

#[test]
fn an_input_that_appears_at_the_root_mid_run_does_not_reach_the_run() {
    let lab = lab();
    // The one input, data/extra.txt, is absent at registration; row 0 writes it at the root.
    let extra = |t: String| t.replace("data/example.txt", "data/extra.txt");
    let root = common::work(&lab, "root-input");
    let at = root.join("data/extra.txt");
    let edit = edits_in_row_0(&format!("printf x > '{}'", at.display()));
    register_study(&lab, "root-input", extra, &[], &edit);
    let plan = plan_of(&lab, "root-input");
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    assert!(at.is_file());
    // The rows ran on the view taken at prepare, which had no such input.
    for r in rows_of(&lab, "root-input") {
        assert_eq!(r.body["data_hashes"], serde_json::json!([]));
    }
}

/// The store path of the blob holding `bytes`.
fn blob_of(lab: &Lab, bytes: &[u8]) -> std::path::PathBuf {
    lab.home.root.join("store").join(sha256_hex(bytes))
}

#[test]
fn a_changed_store_blob_pauses_the_run() {
    let lab = lab();
    let blob = blob_of(&lab, b"calibration");
    let edit = edits_in_row_0(&format!(
        "chmod u+w '{b}'; printf x >> '{b}'",
        b = blob.display()
    ));
    common::with_payload(&lab, "blob", &edit);
    pauses_after_row_0(&lab, "blob", "data/example.txt changed after the run began");
    // The next prepare hashes the blob again and refuses it.
    let err = err_of(&lab, "blob");
    assert!(
        matches!(&err, RunError::InputChanged(p) if p == "data/example.txt"),
        "{err}"
    );
}

#[test]
fn a_missing_blob_refuses_prepare() {
    let lab = lab();
    let dir = plain(&lab, "noblob");
    let blob = blob_of(&lab, b"calibration");
    let aside = blob.with_extension("aside");
    std::fs::rename(&blob, &aside).unwrap();
    let err = err_of(&lab, "noblob");
    assert!(
        matches!(&err, RunError::InputMissing(p) if p == "data/example.txt"),
        "{err}"
    );
    assert!(err.to_string().contains("data/example.txt"), "{err}");
    assert!(!dir.join("schedule.json").exists());
    // The root still holds the file, but a registered input comes from the store only.
    assert!(
        common::work(&lab, "noblob")
            .join("data/example.txt")
            .is_file()
    );
    std::fs::rename(&aside, &blob).unwrap();
    assert_eq!(plan_of(&lab, "noblob").schedule.rows.len(), 6);
}

#[test]
fn a_same_size_rewrite_pauses_the_run() {
    let lab = lab();
    // Row 0 rewrites one byte of the blob in place, one clock tick after the scan: the size
    // stays, the time moves.
    let blob = blob_of(&lab, b"calibration");
    let edit = edits_in_row_0(&format!(
        "sleep 0.03; b='{b}'; chmod u+w \"$b\"; printf C | dd of=\"$b\" bs=1 count=1 conv=notrunc 2>/dev/null",
        b = blob.display()
    ));
    common::with_payload(&lab, "rewrite", &edit);
    pauses_after_row_0(
        &lab,
        "rewrite",
        "data/example.txt changed after the run began",
    );
    assert_eq!(std::fs::read(&blob).unwrap(), b"Calibration");
}

#[test]
fn a_same_size_rewrite_with_its_old_time_put_back_still_pauses() {
    let lab = lab();
    // Row 0 rewrites one byte in place and puts the old time back: only the ctime moves,
    // which no unprivileged writer can set back, so the file is hashed in full.
    let blob = blob_of(&lab, b"calibration");
    let edit = edits_in_row_0(&format!(
        "sleep 0.03; b='{b}'; touch -r \"$b\" \"$IGNORARIUM_OUT/../t\"; chmod u+w \"$b\"; \
         printf C | dd of=\"$b\" bs=1 count=1 conv=notrunc 2>/dev/null; \
         touch -r \"$IGNORARIUM_OUT/../t\" \"$b\"",
        b = blob.display()
    ));
    common::with_payload(&lab, "stamp", &edit);
    pauses_after_row_0(
        &lab,
        "stamp",
        "data/example.txt changed after the run began",
    );
    assert_eq!(std::fs::read(&blob).unwrap(), b"Calibration");
    // A new run hashes every input once and refuses the changed one.
    let err = err_of(&lab, "stamp");
    assert!(
        matches!(&err, RunError::InputChanged(p) if p == "data/example.txt"),
        "{err}"
    );
}

#[test]
fn a_mode_change_on_a_blob_does_not_pause_the_run() {
    let lab = lab();
    // Only the ctime moves; the file is hashed once and its bytes are the same.
    let blob = blob_of(&lab, b"calibration");
    let edit = edits_in_row_0(&format!("chmod u+w '{}'", blob.display()));
    common::with_payload(&lab, "chmod", &edit);
    let plan = plan_of(&lab, "chmod");
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    assert_eq!(rows_of(&lab, "chmod").len(), 6);
    // The next attempt keeps the new stamp, so later attempts do not hash the file again.
    let now = ignorarium::view::scan(&plan.view).unwrap();
    assert_eq!(*plan.view_seen.borrow(), now);
}

#[test]
fn a_name_that_is_not_utf8_added_to_the_view_pauses_the_run() {
    let lab = lab();
    let edit = edits_in_row_0(
        "chmod u+w \"$IGNORARIUM_IN\"; printf x > \"$IGNORARIUM_IN/$(printf '\\377')\"",
    );
    common::with_payload(&lab, "odd-name", &edit);
    pauses_after_row_0(
        &lab,
        "odd-name",
        "the input view changed after the run began",
    );
}

#[test]
fn a_view_that_cannot_be_read_pauses_with_the_cause() {
    let lab = lab();
    let edit = edits_in_row_0("mv \"$IGNORARIUM_IN\" \"$IGNORARIUM_IN.gone\"");
    common::with_payload(&lab, "no-view", &edit);
    let why = format!(
        "the input view cannot be read: {}",
        std::io::Error::from_raw_os_error(2)
    );
    pauses_after_row_0(&lab, "no-view", &why);
}

#[test]
fn a_late_unpinned_input_is_read_from_the_root_and_gated() {
    let lab = lab();
    let dir = register_study(&lab, "late-root", conf_input, &[], PAYLOAD);
    // A file in the box's copy of the study is not an input; the root's file is.
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("data/conf.txt"), "box copy").unwrap();
    let root = common::work(&lab, "late-root");
    std::fs::write(root.join("data/conf.txt"), "Late  Root").unwrap();
    let plan = plan_of(&lab, "late-root");
    let hashes = file_hashes(b"Late  Root");
    assert_eq!(
        plan.inputs,
        vec![
            ("data/example.txt".to_string(), None),
            ("data/conf.txt".to_string(), Some(hashes.clone()))
        ]
    );
    // Imported into the store with a signed record, and seen by rows through the view.
    let view = dir.join("in/data/conf.txt");
    assert_eq!(std::fs::read(&view).unwrap(), b"Late  Root");
    assert_eq!(
        std::fs::read(blob_of(&lab, b"Late  Root")).unwrap(),
        b"Late  Root"
    );
    let key = keys::load(&lab.home).unwrap().unwrap();
    let imports = lab.home.root.join("store/imports.jsonl");
    let recs = records::read_all(&imports, &key.verifying_key()).unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].body["source"], "run late-root");
    assert_eq!(recs[0].body["digest"], hashes.0.as_str());
    // A resume imports the same file again and signs no second record.
    plan_of(&lab, "late-root");
    assert_eq!(
        records::read_all(&imports, &key.verifying_key())
            .unwrap()
            .len(),
        1
    );
    // Gated as late data: one leaf under the study's own label.
    let log = PlanLog::open(&lab.home.root.join("planlog")).unwrap();
    let uses = log.uses(&hashes.0, &hashes.1).unwrap();
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].1.label, "study:late-root");
}

#[test]
fn the_first_prepare_freezes_every_input_and_a_resume_on_other_bytes_is_refused() {
    let lab = lab();
    let dir = register_study(&lab, "frozen", conf_input, &[], PAYLOAD);
    let conf = common::work(&lab, "frozen").join("data/conf.txt");
    std::fs::write(&conf, "first text").unwrap();
    plan_of(&lab, "frozen");
    // One signed record holds each input's digest at the first prepare, or none when absent.
    let first = sha256_hex(b"first text");
    let frozen = kind_of(&lab, "frozen", "inputs");
    assert_eq!(frozen.len(), 1);
    assert_eq!(
        frozen[0].body,
        serde_json::json!({ "inputs": [["data/example.txt", null], ["data/conf.txt", first]] })
    );
    // A resume on the same bytes adds no record.
    plan_of(&lab, "frozen");
    assert_eq!(kind_of(&lab, "frozen", "inputs").len(), 1);
    // Other bytes, the file gone, or an input absent at first that is there now: refused,
    // before the view or the plan log change.
    let view = std::fs::read(dir.join("in/data/conf.txt")).unwrap();
    let example = common::work(&lab, "frozen").join("data/example.txt");
    let log = PlanLog::open(&lab.home.root.join("planlog")).unwrap();
    let second = file_hashes(b"second text");
    let refused = |path: &str| {
        let err = err_of(&lab, "frozen");
        assert!(
            matches!(&err, RunError::InputFrozen(p) if p == path),
            "{err}"
        );
        assert_eq!(
            err.to_string(),
            format!(
                "input {path} differs from what the study's first run read; \
                 put that back, or register the plan again as a new study"
            )
        );
        assert_eq!(std::fs::read(dir.join("in/data/conf.txt")).unwrap(), view);
        assert!(log.uses(&second.0, &second.1).unwrap().is_empty());
    };
    std::fs::write(&conf, "second text").unwrap();
    refused("data/conf.txt");
    std::fs::write(&conf, "first text").unwrap();
    plan_of(&lab, "frozen");
    std::fs::remove_file(&conf).unwrap();
    refused("data/conf.txt");
    std::fs::write(&conf, "first text").unwrap();
    plan_of(&lab, "frozen");
    std::fs::write(&example, "new file").unwrap();
    refused("data/example.txt");
    std::fs::remove_file(&example).unwrap();
    plan_of(&lab, "frozen");
    assert_eq!(kind_of(&lab, "frozen", "inputs").len(), 1);
}

#[test]
fn a_late_input_is_read_only_from_the_registered_root() {
    let lab = lab();
    // data/confirm.txt is absent at registration, so the study reads it from its root.
    plain(&lab, "rooted");
    let other = lab.root.join("elsewhere");
    std::fs::create_dir_all(other.join("data")).unwrap();
    std::fs::write(other.join("data/confirm.txt"), "other text").unwrap();
    let err = prepare_with(&lab.home, "rooted", &other, vec![Box::new(Level0)])
        .err()
        .unwrap();
    assert!(
        matches!(&err, RunError::OtherRoot(p) if p == "data/confirm.txt"),
        "{err}"
    );
    assert!(
        err.to_string().starts_with(
            "input data/confirm.txt was absent at registration and is read from the study root; "
        ),
        "{err}"
    );
    assert!(!lab.home.study_dir("rooted").join("schedule.json").exists());
    assert!(kind_of(&lab, "rooted", "inputs").is_empty());
    // A study whose every input is in the store reads nothing from a root.
    let both = [
        ("data/example.txt", "calibration"),
        ("data/confirm.txt", "held text"),
    ];
    register_study(&lab, "held", |t| t, &both, PAYLOAD);
    prepare_with(&lab.home, "held", &other, vec![Box::new(Level0)]).unwrap();
}

#[test]
fn a_refused_first_prepare_freezes_nothing() {
    let lab = lab();
    // Another study used the late file first: the gate refuses, and the study is not yet
    // tied to those bytes.
    register_study(&lab, "early", conf_input, &[], PAYLOAD);
    register_study(&lab, "later", conf_input, &[], PAYLOAD);
    for id in ["early", "later"] {
        std::fs::write(common::work(&lab, id).join("data/conf.txt"), "shared").unwrap();
    }
    plan_of(&lab, "early");
    assert!(matches!(err_of(&lab, "later"), RunError::DataReused { .. }));
    assert!(kind_of(&lab, "later", "inputs").is_empty());
    std::fs::write(common::work(&lab, "later").join("data/conf.txt"), "own").unwrap();
    plan_of(&lab, "later");
    assert_eq!(kind_of(&lab, "later", "inputs").len(), 1);
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

/// A payload that writes `values` as its values file, and one result file.
fn writes_values(values: &str) -> String {
    format!(
        "#!/bin/sh
printf '%s' '{values}' > \"$IGNORARIUM_OUT/values.json\"
"
    )
}

#[test]
fn a_row_with_bad_values_is_void_and_not_retried() {
    let lab = lab();
    // (payload, the reason must contain)
    let cases = [
        (
            "#!/bin/sh
true
"
            .to_string(),
            "missing",
        ),
        (writes_values(r#"{"ppl": "x"}"#), "number"),
        (writes_values(r#"{"ppl": 1, "extra": 2}"#), "unknown"),
        (writes_values("{"), "JSON"),
        (
            "#!/bin/sh
head -c 70000 /dev/zero | tr '\0' ' ' > \"$IGNORARIUM_OUT/values.json\"
"
            .to_string(),
            "larger",
        ),
        (writes_values(r#"{"ppl": 1, "ppl": 2}"#), "twice"),
        (
            "#!/bin/sh
mkdir \"$IGNORARIUM_OUT/values.json\"
"
            .to_string(),
            "regular",
        ),
    ];
    for (i, (payload, want)) in cases.iter().enumerate() {
        let id = format!("badvalues{i}");
        common::with_payload(&lab, &id, payload);
        assert_eq!(run(&plan_of(&lab, &id)).unwrap(), RunStatus::Done, "{want}");
        let rows = rows_of(&lab, &id);
        assert_eq!(rows.len(), 6, "{want}");
        for r in &rows {
            assert_eq!(r.body["exit"], "void:output", "{want}");
            assert_eq!(r.body["attempts"], 1, "{want}");
            assert!(r.body.get("values").is_none(), "{want}");
            let reason = r.body["reason"].as_str().unwrap_or_default();
            assert!(reason.starts_with("values: "), "{reason}");
            assert!(reason.contains(want), "{reason}");
            // The output was hashed before the values were read.
            assert_eq!(r.body["output_hash"].as_str().unwrap().len(), 64);
        }
        assert!(kind_of(&lab, &id, "spend").is_empty());
    }
}

#[test]
fn ok_rows_record_their_values() {
    let lab = lab();
    common::with_payload(&lab, "hasvalues", &writes_values(r#"{"ppl": 12.5}"#));
    assert_eq!(run(&plan_of(&lab, "hasvalues")).unwrap(), RunStatus::Done);
    let rows = rows_of(&lab, "hasvalues");
    assert_eq!(rows.len(), 6);
    for r in &rows {
        assert_eq!(r.body["exit"], "ok");
        assert_eq!(r.body["values"], serde_json::json!({ "ppl": 12.5 }));
    }
}

#[test]
fn a_failed_payload_records_no_values() {
    let lab = lab();
    let payload = "#!/bin/sh
printf '{\"ppl\": 1}' > \"$IGNORARIUM_OUT/values.json\"
exit 3
";
    common::with_payload(&lab, "failvalues", payload);
    assert_eq!(run(&plan_of(&lab, "failvalues")).unwrap(), RunStatus::Done);
    for r in rows_of(&lab, "failvalues") {
        assert_eq!(r.body["exit"], "payload:3");
        assert!(r.body.get("values").is_none());
    }
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
    fn cpus(&self) -> u32 {
        1
    }
    fn launcher(&self) -> String {
        "process".into()
    }
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
    let plan = prepare_with(
        &home,
        &id,
        &common::work_of(&home, &id),
        vec![Box::new(Level0)],
    )
    .unwrap();
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
    register_study(&lab, "plan-b", conf_input, &[], PAYLOAD);
    for id in ["plan-a", "plan-b"] {
        std::fs::write(common::work(&lab, id).join("data/conf.txt"), "late file").unwrap();
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

/// `staged` as a confirmatory plan, with the sd and icc of each candidate from `my-pilot`.
fn confirmatory_staged(t: String) -> String {
    common::confirmatory(staged(t))
}

/// The pooled estimates `my-pilot` holds for the pick of `staged`: one per candidate.
fn staged_pilot(lab: &common::Lab) {
    let candidate = |id, params| common::estimate(&[(id, 1.0, params)], "", 4.0, Some(0.1));
    common::pilot(
        &lab.home,
        vec![
            candidate("arm-a", serde_json::json!({ "bits": 4, "mode": "fast" })),
            candidate("arm-b", serde_json::json!({ "bits": 8 })),
        ],
    );
}

#[test]
fn a_late_input_with_the_data_of_the_other_role_is_refused() {
    let lab = lab();
    staged_pilot(&lab);
    let (sel, conf) = ("data/select.txt", "data/confirm.txt");
    // (study, files at registration, files written after it). A late exact copy of a
    // registered file has a registered hash, so lateness by hash alone would miss it.
    let cases = [
        (
            "late-sel",
            vec![(conf, "Shared  Text")],
            vec![(sel, "shared text")],
        ),
        (
            "late-conf",
            vec![(sel, "same bytes")],
            vec![(conf, "same bytes")],
        ),
        ("both-late", vec![], vec![(sel, "both"), (conf, "both")]),
    ];
    for (id, before, after) in cases {
        let dir = register_study(&lab, id, confirmatory_staged, &before, PAYLOAD);
        for (p, text) in after {
            std::fs::write(common::work(&lab, id).join(p), text).unwrap();
        }
        let err = prepare_with(
            &lab.home,
            id,
            &common::work_of(&lab.home, id),
            vec![Box::new(Level0)],
        )
        .err()
        .unwrap_or_else(|| panic!("{id}: prepare should refuse"));
        assert_eq!(
            err.to_string(),
            format!(
                "E-CNF-004: input {conf} holds the same data as input {sel}, of the other role. See docs/rules/E-CNF-004.md"
            ),
            "{id}"
        );
        assert!(!dir.join("schedule.json").exists(), "{id}");
    }
    // Other data passes, and so does an exploratory plan, which E-CNF-004 does not cover.
    let work = |id: &str| common::work(&lab, id);
    register_study(&lab, "apart", confirmatory_staged, &[], PAYLOAD);
    std::fs::write(work("apart").join(sel), "one text").unwrap();
    std::fs::write(work("apart").join(conf), "another text").unwrap();
    plan_of(&lab, "apart");
    register_study(&lab, "loose", staged, &[], PAYLOAD);
    std::fs::write(work("loose").join(sel), "both").unwrap();
    std::fs::write(work("loose").join(conf), "both").unwrap();
    plan_of(&lab, "loose");
}

#[test]
fn a_late_near_copy_of_the_studys_own_data_is_not_an_earlier_use() {
    let lab = lab();
    let (sel, conf) = ("data/select.txt", "data/confirm.txt");
    // An exploratory plan may share data between roles. The late selection file is the
    // registered confirmation text in other case and spacing: its normalized hash is in the
    // study's own plan leaf, which is no earlier study.
    register_study(&lab, "own", staged, &[(conf, "Own  Text")], PAYLOAD);
    std::fs::write(common::work(&lab, "own").join(sel), "own text").unwrap();
    let plan = plan_of(&lab, "own");
    let late = file_hashes(b"own text");
    assert!(
        plan.inputs
            .iter()
            .any(|(p, h)| p == sel && h.as_ref() == Some(&late))
    );
    // Another study's use of the same text is still refused.
    register_study(&lab, "other", staged, &[], PAYLOAD);
    std::fs::write(common::work(&lab, "other").join(sel), "OWN text").unwrap();
    match err_of(&lab, "other") {
        RunError::DataReused { path, label } => {
            assert_eq!(path, sel);
            assert_eq!(label, "registered plan, leaf 0");
        }
        e => panic!("{e}"),
    }
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
    register_study(&lab, "mnist-ablation", conf_input, &[], PAYLOAD);
    let late = common::work(&lab, "mnist-ablation").join("data/conf.txt");
    std::fs::write(late, "held-out rows").unwrap();
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
    register_study(&lab, "late", conf_input, &[], PAYLOAD);
    let late = common::work(&lab, "late").join("data/conf.txt");
    std::fs::write(late, "Late  File").unwrap();
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
    register_study(
        &lab,
        "early",
        conf_input,
        &[("data/conf.txt", "known")],
        PAYLOAD,
    );
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
    let none = prepare_with(
        &lab.home,
        "guard",
        &common::work_of(&lab.home, "guard"),
        vec![],
    )
    .err()
    .unwrap();
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
fn a_payload_copy_that_differs_from_the_register_record_is_refused() {
    let lab = lab();
    let dir = plain(&lab, "copied");
    let registered = kind_of(&lab, "copied", "register")[0].body["payload"].clone();
    assert_eq!(
        plan_of(&lab, "copied").payload_sha256,
        registered.as_str().unwrap()
    );
    // The box's copy is read-only; an owner can still change it.
    let copy = dir.join("payload");
    common::unseal(&copy);
    std::fs::write(copy.join("extra"), "").unwrap();
    let err = err_of(&lab, "copied");
    assert!(matches!(err, RunError::PayloadChanged), "{err}");
    assert_eq!(
        err.to_string(),
        "the payload differs from the registered copy"
    );
}

#[test]
fn the_schedule_runs_the_picked_units_of_the_confirmation_input() {
    let lab = lab();
    plain(&lab, "units");
    let plan = plan_of(&lab, "units");
    let rows = &plan.schedule.rows;
    assert_eq!(rows, &schedule::rows(&plan.spec, &plan.spec_hash).unwrap());
    let got: Vec<_> = rows
        .iter()
        .map(|r| {
            (
                &r.unit[..],
                &r.cluster[..],
                &r.stratum[..],
                &r.condition[..],
            )
        })
        .collect();
    let c = "data/confirm.txt";
    assert_eq!(
        got,
        [
            ("data/confirm.txt#0", c, "fiction", "exact"),
            ("data/confirm.txt#0", c, "fiction", "arm-a"),
            ("data/confirm.txt#1", c, "fiction", "exact"),
            ("data/confirm.txt#1", c, "fiction", "arm-a"),
            ("data/confirm.txt#2", c, "fiction", "exact"),
            ("data/confirm.txt#2", c, "fiction", "arm-a"),
        ]
    );
    // The estimate a row is held to counts these rows.
    let m = ignorarium::machine::load(&lab.home.root).unwrap();
    assert_eq!(estimate::estimate(&plan.spec, &m, rows).rows, 6);
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
fn the_highest_backend_that_meets_min_isolation_runs() {
    let lab = lab();
    plain(&lab, "lvl0");
    register_study(&lab, "lvl1", need_isolation, &[], PAYLOAD);
    let pick = |id: &str, levels: &[u8]| {
        let all = levels.iter().map(|&l| fake_at(l)).collect();
        prepare_with(&lab.home, id, &common::work_of(&lab.home, id), all)
            .unwrap()
            .backend
            .level()
    };
    assert_eq!(pick("lvl0", &[2, 0, 1]), 2);
    assert_eq!(pick("lvl0", &[0, 1]), 1);
    assert_eq!(pick("lvl0", &[1, 0]), 1);
    assert_eq!(pick("lvl0", &[0]), 0);
    assert_eq!(pick("lvl1", &[2, 0, 1]), 2);
    assert_eq!(pick("lvl1", &[0, 1]), 1);
    let only0 = prepare_with(
        &lab.home,
        "lvl1",
        &common::work_of(&lab.home, "lvl1"),
        vec![fake_at(0)],
    )
    .err()
    .unwrap();
    assert!(
        matches!(&only0, RunError::NoBackend { need: 1, have } if *have == [0]),
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
    assert_eq!(
        prepare(&lab.home, "reg", &common::work_of(&lab.home, "reg"), 0)
            .unwrap()
            .backend
            .level(),
        0
    );
    register_study(&lab, "reg1", need_isolation, &[], PAYLOAD);
    let err = prepare(&lab.home, "reg1", &common::work_of(&lab.home, "reg1"), 0)
        .err()
        .unwrap();
    assert!(matches!(err, RunError::NoBackend { need: 1, .. }), "{err}");
}

#[test]
fn a_symlinked_payload_is_refused() {
    let lab = lab();
    let dir = plain(&lab, "link-payload");
    let outside = lab.root.join("outside-payload");
    std::fs::write(&outside, PAYLOAD).unwrap();
    common::unseal(&dir.join("payload"));
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
    assert!(matches!(err, RunError::BadRegister(_)), "{err}");
}

#[test]
fn a_register_record_without_a_root_is_refused_as_bad() {
    let lab = lab();
    let dir = plain(&lab, "noroot");
    let key = keys::load(&lab.home).unwrap().unwrap();
    let log = dir.join("records.jsonl");
    std::fs::rename(&log, dir.join("records.old")).unwrap();
    let hash = spec::load(&dir.join("spec.toml")).unwrap().hash;
    let draft = records::RecordDraft {
        study: "noroot".into(),
        row: None,
        kind: "register".into(),
        body: serde_json::json!({"spec_hash": hash, "leaf": 1, "log": "planlog", "defaults": []}),
    };
    records::append(&log, &key, draft).unwrap();
    let err = err_of(&lab, "noroot");
    assert!(matches!(err, RunError::BadRegister(_)), "{err}");
    assert!(err.to_string().contains("does not parse"), "{err}");
}

#[test]
fn a_reused_input_names_a_registered_plan_by_leaf() {
    let lab = lab();
    register_study(&lab, "plan-a", conf_input, &[], PAYLOAD);
    register_study(
        &lab,
        "plan-c",
        conf_input,
        &[("data/conf.txt", "shared")],
        PAYLOAD,
    );
    std::fs::write(common::work(&lab, "plan-a").join("data/conf.txt"), "shared").unwrap();
    match err_of(&lab, "plan-a") {
        RunError::DataReused { label, .. } => assert_eq!(label, "registered plan, leaf 1"),
        e => panic!("{e}"),
    }
}

#[test]
fn a_plan_leaf_labelled_like_the_study_is_not_its_own_use() {
    let lab = lab();
    // A plan leaf carries the earnable label `signal`; a study with that id must not match it.
    register_study(&lab, "signal", conf_input, &[], PAYLOAD);
    register_study(
        &lab,
        "other",
        conf_input,
        &[("data/conf.txt", "shared")],
        PAYLOAD,
    );
    std::fs::write(common::work(&lab, "signal").join("data/conf.txt"), "shared").unwrap();
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
    // Keep the register and inputs records and rows 0 and 1: 2000 s of 3600 s, past 0.50.
    let log = dir.join("records.jsonl");
    let text = std::fs::read_to_string(&log).unwrap();
    let kept: String = text.split_inclusive('\n').take(4).collect();
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
fn a_plan_run_twice_counts_each_attempt_once() {
    let lab = lab();
    plain(&lab, "twice");
    let mut plan = plan_of(&lab, "twice");
    // The first session ends in a crash after rows 0..2; the same plan then runs again.
    let (mut b, _) = fake(vec![]);
    b.wall = Some(100.0);
    b.panic_at = Some(3);
    plan.backend = Box::new(b);
    assert!(catch_unwind(AssertUnwindSafe(|| run(&plan))).is_err());
    let (mut b, _) = fake(vec![]);
    b.wall = Some(100.0);
    plan.backend = Box::new(b);
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let recorded: f64 = common::records_of(&lab, "twice")
        .iter()
        .filter(|r| r.kind == "row" || r.kind == "spend")
        .map(|r| r.body["cpu_seconds"].as_f64().unwrap())
        .sum();
    assert_eq!(recorded, 300.0);
    assert_eq!(plan.meter.sample().unwrap().cpu_seconds, recorded);
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
    // Keep the register and inputs records and rows 0..3, as a crash would.
    let log = dir.join("records.jsonl");
    let text = std::fs::read_to_string(&log).unwrap();
    let kept: String = text.split_inclusive('\n').take(5).collect();
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

#[test]
fn run_start_names_the_isolation_and_the_launcher() {
    let lab = lab();
    let dir = plain(&lab, "starts");
    assert_eq!(run(&plan_of(&lab, "starts")).unwrap(), RunStatus::Done);
    let ev = events_of(&dir);
    let starts = named(&ev, "run.start");
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0]["attrs"]["isolation"], 0);
    assert_eq!(starts[0]["attrs"]["launcher"], "process");
}

#[test]
fn a_resumed_run_writes_run_resume_and_no_second_run_start() {
    let lab = lab();
    let dir = plain(&lab, "resumes");
    let mut plan = plan_of(&lab, "resumes");
    let infra = || Exit::Infra("box fault".into());
    plan.backend = Box::new(fake(vec![infra(), infra(), infra(), infra()]).0);
    assert!(matches!(run(&plan).unwrap(), RunStatus::Paused(_)));
    let ev = events_of(&dir);
    assert_eq!(named(&ev, "run.start").len(), 1);
    assert!(named(&ev, "run.resume").is_empty());
    assert_eq!(run(&plan_of(&lab, "resumes")).unwrap(), RunStatus::Done);
    let ev = events_of(&dir);
    assert_eq!(named(&ev, "run.start").len(), 1);
    let resumes = named(&ev, "run.resume");
    assert_eq!(resumes.len(), 1);
    assert_eq!(resumes[0]["attrs"]["isolation"], 0);
    assert_eq!(resumes[0]["attrs"]["launcher"], "process");
}

fn outcome(cpu: f64, wall: f64, ended: bool, unknown: bool) -> RowOutcome {
    RowOutcome {
        exit: Exit::Ok,
        cpu_seconds: cpu,
        wall_seconds: wall,
        output_hash: String::new(),
        selftest: None,
        cpu_unknown: unknown.then(|| "unreadable".to_string()),
        ended,
    }
}

#[test]
fn charge_takes_the_bound_only_for_ended_or_unknown_rows() {
    use ignorarium::runner::charge;
    assert_eq!(
        charge(&outcome(2.0, 10.0, true, false), 8, 100.0),
        (80.0, true)
    );
    // The row estimate floors only an unknown count.
    assert_eq!(
        charge(&outcome(2.0, 10.0, true, false), 1, 100.0),
        (10.0, true)
    );
    assert_eq!(
        charge(&outcome(2.0, 10.0, false, true), 8, 100.0),
        (100.0, true)
    );
    assert_eq!(
        charge(&outcome(2.0, 10.0, false, true), 8, 50.0),
        (80.0, true)
    );
    // Measured CPU above the bound stays.
    assert_eq!(
        charge(&outcome(90.0, 10.0, true, false), 8, 0.0),
        (90.0, true)
    );
    assert_eq!(
        charge(&outcome(2.0, 10.0, false, false), 8, 100.0),
        (2.0, false)
    );
}

#[test]
fn a_stopped_row_is_charged_wall_times_cpus() {
    const BUSY: &str = "#!/bin/sh\n\
        for i in 1 2 3 4; do (while :; do :; done) & done\n\
        echo started > \"$IGNORARIUM_OUT/started\"\n\
        sleep 30\n";
    let lab = lab();
    let dir = common::with_payload(&lab, "busy-stop", BUSY);
    let plan = plan_of(&lab, "busy-stop");
    let marker = dir.join("run/rows/0/out/started");
    let control = ignorarium::control::ControlFile::of(&lab.home, "busy-stop");
    let other = std::thread::spawn(move || {
        let t = std::time::Instant::now();
        while !marker.exists() {
            assert!(t.elapsed().as_secs() < 20, "row 0 never started");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
        control.request_stop("manual").unwrap();
    });
    assert_eq!(run(&plan).unwrap(), RunStatus::Stopped("manual".into()));
    other.join().unwrap();
    let spends = kind_of(&lab, "busy-stop", "spend");
    assert_eq!(spends.len(), 1);
    let wall = spends[0].body["wall_seconds"].as_f64().unwrap();
    let cpu = spends[0].body["cpu_seconds"].as_f64().unwrap();
    assert_eq!(spends[0].body["cpu_bound"], true);
    assert!(wall >= 1.0, "{wall}");
    assert!(cpu >= wall * f64::from(Level0.cpus()), "{cpu} {wall}");
}

#[test]
fn a_row_that_runs_to_its_end_has_no_bound() {
    let lab = lab();
    plain(&lab, "no-bound");
    let plan = plan_of(&lab, "no-bound");
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    for r in rows_of(&lab, "no-bound") {
        assert!(r.body.get("cpu_bound").is_none());
    }
}

#[test]
fn a_timed_out_row_record_carries_the_bound() {
    let lab = lab();
    plain(&lab, "wall-out");
    let mut plan = plan_of(&lab, "wall-out");
    let (b, _) = fake(vec![]);
    plan.backend = Box::new(Ended(b));
    assert!(matches!(
        run(&plan).unwrap(),
        RunStatus::Done | RunStatus::Paused(_)
    ));
    let rows = rows_of(&lab, "wall-out");
    assert_eq!(rows[0].body["cpu_bound"], true);
    assert_eq!(rows[0].body["cpu_seconds"].as_f64().unwrap(), 40.0);
}

/// Marks every row the box ended after 8 CPUs' worth of wall time.
struct Ended(Fake);
impl Backend for Ended {
    fn cpus(&self) -> u32 {
        8
    }
    fn launcher(&self) -> String {
        self.0.launcher()
    }
    fn level(&self) -> u8 {
        0
    }
    fn probe(&self) -> Probe {
        Probe::Available
    }
    fn run_row(&self, job: &RowJob, watch: &dyn Watch) -> std::io::Result<RowOutcome> {
        let mut o = self.0.run_row(job, watch)?;
        o.exit = Exit::Payload(124);
        o.ended = true;
        o.wall_seconds = 5.0;
        o.cpu_seconds = 1.0;
        Ok(o)
    }
}

// ---- a finished run records its estimates ----

/// The template with a second hypothesis, `H2`, over the same contrast: two estimates.
fn two_hypotheses(t: String) -> String {
    let h2 = r#"[[question.hypotheses]]
id = "H2"
statement = ""
rival = ""
outcome = "ppl-ratio"
contrast = { arm-a = 1.0 }
strata = []

[[outcomes]]"#;
    t.replacen("[[outcomes]]", h2, 1)
}

/// Kinds of the records after the last row record.
fn after_the_rows(lab: &Lab, id: &str) -> Vec<String> {
    let all = common::records_of(lab, id);
    let last = all.iter().rposition(|r| r.kind == "row").unwrap();
    all[last + 1..].iter().map(|r| r.kind.clone()).collect()
}

/// Keep the first `n` records of the log at `path`: the log a crash after record `n` leaves.
fn keep_lines(path: &std::path::Path, n: usize) {
    let text = std::fs::read_to_string(path).unwrap();
    std::fs::write(path, text.split_inclusive('\n').take(n).collect::<String>()).unwrap();
}

/// Keep the first `n` records of the study's log.
fn keep_records(dir: &std::path::Path, n: usize) {
    keep_lines(&dir.join("records.jsonl"), n);
}

/// True when the study is finished as a later study reads it, its stream slot included.
fn finished(lab: &Lab, id: &str) -> bool {
    let key = keys::load(&lab.home).unwrap().unwrap();
    let closed = |log: &str, slot| {
        let log = LogName::parse(log).map_err(|e| e.to_string())?;
        (Log::of(&lab.home, log).closed(&key.verifying_key(), id, slot)).map_err(|e| e.to_string())
    };
    let state = StudyState::of(Some(common::records_of(lab, id)), closed).unwrap();
    matches!(state, StudyState::Finished(_))
}

#[test]
fn a_finished_run_writes_each_record_once_and_heals_a_crash() {
    let lab = lab();
    let dir = register_study(
        &lab,
        "fin",
        two_hypotheses,
        &[("data/example.txt", "calibration")],
        PAYLOAD,
    );
    let plan = plan_of(&lab, "fin");
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    // Register, inputs, 6 rows, the analysis record, one estimate per hypothesis, then one verdict per
    // hypothesis.
    let kinds = ["analysis", "estimate", "estimate", "verdict", "verdict"];
    assert_eq!(after_the_rows(&lab, "fin"), kinds);
    assert!(finished(&lab, "fin"));
    let all = common::records_of(&lab, "fin");
    let a = Analysis::of(&all[8]).unwrap();
    let version = env!("CARGO_PKG_VERSION");
    assert_eq!(
        (a.box_version.as_str(), a.estimates, a.verdicts, a.slot),
        (version, 2, 2, None)
    );
    // Neither hypothesis has a claim: each gets verdict `none`.
    let verdicts = |all: &[records::Record]| -> Vec<(String, String)> {
        (all[11..].iter())
            .map(|r| Verdict::of(r).unwrap())
            .map(|v| (v.hypothesis, v.verdict))
            .collect()
    };
    let none = |h: &str| (h.to_string(), "none".to_string());
    assert_eq!(verdicts(&all), [none("H1"), none("H2")]);
    // The commit hashes the stored spec's question, outcomes and design, then the version.
    let mut stored = spec::load(&dir.join("spec.toml")).unwrap().value;
    estimate::apply_defaults(&mut stored);
    assert_eq!(a.commit, analysis::commit(&stored, version).unwrap());
    assert_eq!(a.commit.len(), 64);
    stored["design"]["power"] = serde_json::json!(0.95);
    assert_ne!(a.commit, analysis::commit(&stored, version).unwrap());
    assert_ne!(a.commit, analysis::commit(&stored, "9.9.9").unwrap());
    let es: Vec<Estimate> = all[9..11]
        .iter()
        .map(|r| Estimate::of(r).unwrap())
        .collect();
    assert_eq!(es[0].hypothesis, "H1");
    assert_eq!(es[1].hypothesis, "H2");
    for e in &es {
        // 3 units in 1 cluster, all paired to 0 %: no icc.
        assert_eq!((e.stratum.as_str(), e.clusters, e.units), ("", 1, 3));
        assert_eq!((e.mean, e.var, e.icc), (0.0, 0.0, None));
    }
    // A second run adds nothing.
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    assert_eq!(common::records_of(&lab, "fin").len(), 13);
    // A crash after any record of the tail: the next run adds what is missing, once.
    for kept in [13, 12, 11, 10, 9] {
        keep_records(&dir, kept);
        assert_eq!(finished(&lab, "fin"), kept == 13, "kept {kept}");
        assert_eq!(run(&plan).unwrap(), RunStatus::Done);
        assert_eq!(after_the_rows(&lab, "fin"), kinds, "kept {kept}");
        assert!(finished(&lab, "fin"));
        let all = common::records_of(&lab, "fin");
        assert_eq!(
            all[9..11]
                .iter()
                .map(|r| Estimate::of(r).unwrap().hypothesis)
                .collect::<Vec<_>>(),
            ["H1", "H2"],
            "kept {kept}"
        );
        assert_eq!(verdicts(&all), [none("H1"), none("H2")], "kept {kept}");
    }
    // A crash after the last row and before the analysis record: it is written first.
    keep_records(&dir, 8);
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    assert_eq!(after_the_rows(&lab, "fin"), kinds);
}

#[test]
fn a_run_with_a_row_missing_writes_no_analysis() {
    let lab = lab();
    plain(&lab, "unfin");
    let mut plan = plan_of(&lab, "unfin");
    let (mut b, _) = fake(vec![]);
    b.panic_at = Some(5);
    plan.backend = Box::new(b);
    assert!(catch_unwind(AssertUnwindSafe(|| run(&plan))).is_err());
    assert_eq!(indices(&lab, "unfin"), vec![0, 1, 2, 3, 4]);
    // Not even a direct call writes results while a row has no record.
    analysis::finish(&plan).unwrap();
    assert!(kind_of(&lab, "unfin", "analysis").is_empty());
    assert!(kind_of(&lab, "unfin", "estimate").is_empty());
}

#[test]
fn a_staged_run_keys_its_estimate_by_the_pick_and_its_params() {
    let lab = lab();
    register_study(&lab, "keyed", staged, &[], STAGED);
    assert_eq!(run(&plan_of(&lab, "keyed")).unwrap(), RunStatus::Done);
    let es = kind_of(&lab, "keyed", "estimate");
    assert_eq!(es.len(), 1);
    let e = Estimate::of(&es[0]).unwrap();
    let terms = [("arm-b".to_string(), 1.0, serde_json::json!({ "bits": 8 }))];
    let reference = ("exact".to_string(), serde_json::json!({}));
    let (key, hash) = contrast_key(&common::ppl_ratio(), 1, &reference, &terms);
    assert_eq!((e.key, e.key_hash), (key, hash));
    // The pick pairs to -25 % on every confirmation unit.
    assert_eq!((e.clusters, e.units, e.mean, e.var), (1, 3, -25.0, 0.0));
}

/// Sign a record of `kind` with `body` onto the study's log.
fn append_record(lab: &Lab, id: &str, kind: &str, body: serde_json::Value) {
    let key = keys::load(&lab.home).unwrap().unwrap();
    let draft = records::RecordDraft {
        study: id.into(),
        row: None,
        kind: kind.into(),
        body,
    };
    let log = lab.home.study_dir(id).join("records.jsonl");
    records::append(&log, &key, draft).unwrap();
}

/// A study with two hypotheses, run to the end: its dir, its plan and its records.
fn finished_study(lab: &Lab, id: &str) -> (std::path::PathBuf, RunPlan, Vec<records::Record>) {
    let dir = register_study(
        lab,
        id,
        two_hypotheses,
        &[("data/example.txt", "calibration")],
        PAYLOAD,
    );
    let plan = plan_of(lab, id);
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    (dir, plan, common::records_of(lab, id))
}

#[test]
fn an_analysis_record_that_does_not_parse_is_an_error() {
    let lab = lab();
    let (dir, plan, _) = finished_study(&lab, "badan");
    keep_records(&dir, 8);
    append_record(
        &lab,
        "badan",
        "analysis",
        serde_json::json!({ "commit": 1 }),
    );
    match run(&plan) {
        Err(RunError::Records(why)) => assert!(why.contains("analysis"), "{why}"),
        other => panic!("{other:?}"),
    }
    // Nothing was written after it.
    assert_eq!(common::records_of(&lab, "badan").len(), 9);
}

#[test]
fn finish_refuses_when_the_spec_changed_since_registration() {
    let lab = lab();
    let (dir, plan, _) = finished_study(&lab, "moved");
    keep_records(&dir, 8);
    let path = dir.join("spec.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("power = 0.8", "power = 0.9")).unwrap();
    assert!(matches!(
        analysis::finish(&plan),
        Err(RunError::SpecChanged)
    ));
    assert_eq!(common::records_of(&lab, "moved").len(), 8);
}

#[test]
fn a_heal_by_another_box_version_is_refused_only_when_it_adds_something() {
    let lab = lab();
    let (dir, plan, all) = finished_study(&lab, "vers");
    let old = |a: &records::Record| {
        let mut b = a.body.clone();
        b["box_version"] = serde_json::json!("0.0.0");
        b
    };
    // The old box wrote the analysis record and one of the two estimates.
    keep_records(&dir, 8);
    append_record(&lab, "vers", "analysis", old(&all[8]));
    append_record(&lab, "vers", "estimate", all[9].body.clone());
    match run(&plan) {
        Err(RunError::Records(why)) => {
            assert!(
                why.contains("0.0.0") && why.contains(env!("CARGO_PKG_VERSION")),
                "{why}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(common::records_of(&lab, "vers").len(), 10);
    // With every estimate there and a verdict missing, it is still refused.
    append_record(&lab, "vers", "estimate", all[10].body.clone());
    append_record(&lab, "vers", "verdict", all[11].body.clone());
    assert!(matches!(run(&plan), Err(RunError::Records(_))));
    assert_eq!(common::records_of(&lab, "vers").len(), 12);
    // With every record there, nothing is added and nothing is refused.
    append_record(&lab, "vers", "verdict", all[12].body.clone());
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    assert_eq!(common::records_of(&lab, "vers").len(), 13);
}

/// `staged`, with arm-b's `bits` taken from study `chooser`.
fn choosing(t: String) -> String {
    staged(t).replace(
        "params = { bits = 8 }",
        "params = { bits = { from = \"chooser\" } }",
    )
}

/// The study `id` in the lab's box, finished, with the selection of `selected`.
fn chose(lab: &Lab, id: &str, selected: &str) -> Vec<String> {
    common::plant(&lab.home, id, &common::finished_selecting(selected, vec![]))
}

#[test]
fn the_payload_gets_the_resolved_param() {
    let lab = lab();
    chose(&lab, "chooser", "arm-a");
    let dir = register_study(&lab, "taking", choosing, &[], STAGED);
    let plan = plan_of(&lab, "taking");
    assert_eq!(plan.params["exact"], serde_json::json!({}));
    assert_eq!(
        plan.params["arm-a"],
        serde_json::json!({ "bits": 4, "mode": "fast" })
    );
    assert_eq!(plan.params["arm-b"], serde_json::json!({ "bits": "arm-a" }));
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    for row in &plan.schedule.rows {
        let got = std::fs::read_to_string(dir.join(format!("run/rows/{}/out/params", row.index)))
            .unwrap();
        let want = match row.condition.as_str() {
            "exact" => "{}",
            "arm-a" => r#"{"bits":4,"mode":"fast"}"#,
            _ => r#"{"bits":"arm-a"}"#,
        };
        assert_eq!(got, want, "row {}", row.index);
    }
}

#[test]
fn prepare_refuses_when_a_frozen_record_is_gone() {
    let lab = lab();
    let planted = chose(&lab, "chooser", "arm-a");
    register_study(&lab, "taking", choosing, &[], STAGED);
    plan_of(&lab, "taking");
    // The parent's folder moves away; the box no longer holds the record the value came from.
    std::fs::rename(lab.home.study_dir("chooser"), lab.root.join("moved")).unwrap();
    match err_of(&lab, "taking") {
        RunError::Records(why) => assert_eq!(why, format!("frozen record {} is gone", planted[1])),
        other => panic!("{other:?}"),
    }
}

#[test]
fn estimate_keys_match_across_studies_with_the_same_resolved_params() {
    let lab = lab();
    let mut hashes = Vec::new();
    for (study, parent, selected) in [
        ("first", "chooser-1", "arm-a"),
        ("second", "chooser-2", "arm-a"),
        ("third", "chooser-3", "arm-b"),
    ] {
        chose(&lab, parent, selected);
        let from = format!("params = {{ bits = {{ from = \"{parent}\" }} }}");
        let edit = move |t: String| staged(t).replace("params = { bits = 8 }", &from);
        register_study(&lab, study, edit, &[], STAGED);
        assert_eq!(run(&plan_of(&lab, study)).unwrap(), RunStatus::Done);
        let es = kind_of(&lab, study, "estimate");
        assert_eq!(es.len(), 1);
        let e = Estimate::of(&es[0]).unwrap();
        // The key holds the selected condition, never the unresolved reference.
        let terms = [(
            "arm-b".to_string(),
            1.0,
            serde_json::json!({ "bits": selected }),
        )];
        let reference = ("exact".to_string(), serde_json::json!({}));
        let (key, hash) = contrast_key(&common::ppl_ratio(), 1, &reference, &terms);
        assert_eq!((e.key, e.key_hash.clone()), (key, hash));
        hashes.push(e.key_hash);
    }
    assert_eq!(hashes[0], hashes[1]);
    assert_ne!(hashes[0], hashes[2]);
}

// ---- a finished run records a verdict per hypothesis and closes its stream slot ----

/// `common::confirmatory` with one unit per cluster: 15 clusters, 30 rows. Its claim: below 0
/// by the median.
fn one_unit_each(t: String) -> String {
    common::confirmatory(t).replace("units = 3", "units = 1")
}

/// A payload whose `arm-a` value is `v` against the reference's 8: 6 pairs to -25 %, 10 to
/// +25 %.
fn arm_a_at(v: u32) -> String {
    format!(
        "#!/bin/sh\ncase \"$2\" in exact) v=8;; *) v={v};; esac\nprintf '{{\"ppl\": %s}}' \"$v\" > \"$IGNORARIUM_OUT/values.json\"\n"
    )
}

/// The records of the stream of `log`, as (kind, body).
fn stream_of(lab: &Lab, log: LogName) -> Vec<(String, serde_json::Value)> {
    let key = keys::load(&lab.home).unwrap().unwrap();
    let path = Log::of(&lab.home, log).path().to_path_buf();
    (records::read_all(&path, &key.verifying_key())
        .unwrap()
        .into_iter())
    .map(|r| (r.kind, r.body))
    .collect()
}

/// The confirmatory study `id` of `repeats` repeats, its spec edited by `edit` after
/// `one_unit_each`, registered in a box whose `my-pilot` gives the sd and icc, then run to the
/// end with `payload` on `backend`.
fn tested(
    lab: &Lab,
    id: &str,
    repeats: u32,
    edit: impl Fn(String) -> String,
    payload: &str,
    backend: Box<dyn Backend>,
) -> RunPlan {
    // The pilot's estimate of the contrast, keyed by the repeats.
    let mut pilot = common::arm_a();
    let terms = [("arm-a".to_string(), 1.0, serde_json::json!({}))];
    let reference = ("exact".to_string(), serde_json::json!({}));
    let (key, hash) = contrast_key(&common::ppl_ratio(), repeats.into(), &reference, &terms);
    (pilot["key"], pilot["key_hash"]) = (key, serde_json::json!(hash));
    common::pilot(&lab.home, vec![pilot]);
    let edit = |t: String| {
        let t = one_unit_each(t).replace("repeats = 1\n", &format!("repeats = {repeats}\n"));
        edit(t)
    };
    register_study(lab, id, edit, &[], payload);
    let mut plan = plan_of(lab, id);
    plan.backend = backend;
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    plan
}

/// The one verdict record of the study.
fn verdict_of(lab: &Lab, id: &str) -> Verdict {
    let vs = kind_of(lab, id, "verdict");
    assert_eq!(vs.len(), 1);
    Verdict::of(&vs[0]).unwrap()
}

/// The level of the stream slot the study's register record holds.
fn level_of(lab: &Lab, id: &str) -> f64 {
    let register = Register::of(&kind_of(lab, id, "register")[0]).unwrap();
    register.alpha.unwrap().level
}

fn close_of(id: &str, e: f64, discovery: bool) -> (String, serde_json::Value) {
    let body = serde_json::json!({ "slot": 1, "study": id, "e_value": e, "discovery": discovery });
    ("stream.close".into(), body)
}

#[test]
fn a_planted_effect_passes_and_closes_a_discovery() {
    let lab = lab();
    // Every unit pairs to -25 %, below the margin 0: each of the 15 clusters scores 1, and the
    // 14 bets of 1/2 give 1.5^14 = 292, past 1 / 0.005.
    tested(&lab, "planted", 1, |t| t, &arm_a_at(6), Box::new(Level0));
    let v = verdict_of(&lab, "planted");
    let level = level_of(&lab, "planted");
    let e = v.e_value.unwrap();
    assert!(
        (e - 1.5f64.powi(14)).abs() < 1e-9 && e >= 1.0 / level,
        "{e}"
    );
    let want = Verdict {
        hypothesis: "H1".into(),
        verdict: "pass".into(),
        basis: "test".into(),
        level: Some(level),
        e_value: Some(e),
        bounds: vec![BoundResult {
            stratum: "".into(),
            side: "below".into(),
            margin: 0.0,
            clusters: 15,
            value: Some(e),
        }],
        incomplete_units: 0,
        reason: None,
    };
    assert_eq!(v, want);
    // The slot closes with the study's e-value: a discovery at its level.
    let stream = stream_of(&lab, LogName::Plan);
    assert_eq!(stream.last(), Some(&close_of("planted", e, true)));
    assert!(finished(&lab, "planted"));
}

#[test]
fn a_null_study_fails_and_closes_without_a_discovery() {
    let lab = lab();
    // Every unit pairs to +25 %: no cluster lies below the margin, and the test never bets.
    // The study registers into the test log, and its slot closes there.
    let test_log = |t: String| t.replace("test_log = false", "test_log = true");
    tested(&lab, "null", 1, test_log, &arm_a_at(10), Box::new(Level0));
    let v = verdict_of(&lab, "null");
    assert_eq!(
        (v.verdict.as_str(), v.e_value, v.reason),
        ("fail", Some(1.0), None)
    );
    let stream = stream_of(&lab, LogName::Test);
    assert_eq!(stream.last(), Some(&close_of("null", 1.0, false)));
    assert!(stream_of(&lab, LogName::Plan).is_empty());
    assert!(finished(&lab, "null"));
}

#[test]
fn a_void_row_in_scope_fails_the_claim() {
    let lab = lab();
    // The planted effect, with row 0 void: its unit has no paired value.
    let (b, _) = fake(vec![Exit::Payload(1)]);
    tested(&lab, "void", 1, |t| t, &arm_a_at(6), Box::new(b));
    assert_eq!(rows_of(&lab, "void")[0].body["exit"], "payload:1");
    let v = verdict_of(&lab, "void");
    assert_eq!(
        (v.verdict.as_str(), v.e_value, v.incomplete_units),
        ("fail", Some(0.0), 1)
    );
    assert_eq!(v.reason.as_deref(), Some("incomplete units in scope: 1"));
    // The bound reports the test on the 14 complete clusters.
    assert_eq!(v.bounds[0].clusters, 14);
    let e = v.bounds[0].value.unwrap();
    assert!((e - 1.5f64.powi(13)).abs() < 1e-9, "{e}");
    // The slot still closes, with e-value 0.
    let stream = stream_of(&lab, LogName::Plan);
    assert_eq!(stream.last(), Some(&close_of("void", 0.0, false)));
    assert!(finished(&lab, "void"));

    // Two repeats: row 0, the first unit's reference row of repeat 0, is void. Its repeat 1
    // still pairs, so the unit has a value, and it is still incomplete: it leaves the test
    // with its cluster.
    let lab = common::lab();
    let (b, _) = fake(vec![Exit::Payload(1)]);
    let plan = tested(&lab, "void2", 2, |t| t, &arm_a_at(6), Box::new(b));
    let row0 = &plan.schedule.rows[0];
    assert_eq!((row0.condition.as_str(), row0.repeat), ("exact", 0));
    assert_eq!(rows_of(&lab, "void2")[0].body["exit"], "payload:1");
    let v = verdict_of(&lab, "void2");
    assert_eq!(
        (v.verdict.as_str(), v.e_value, v.incomplete_units),
        ("fail", Some(0.0), 1)
    );
    assert_eq!(v.reason.as_deref(), Some("incomplete units in scope: 1"));
    assert_eq!(v.bounds[0].clusters, 14);
    let stream = stream_of(&lab, LogName::Plan);
    assert_eq!(stream.last(), Some(&close_of("void2", 0.0, false)));
}

#[test]
fn verdicts_and_the_close_are_written_once() {
    let lab = lab();
    let plan = tested(&lab, "once", 1, |t| t, &arm_a_at(6), Box::new(Level0));
    let dir = lab.home.study_dir("once");
    let stream = Log::of(&lab.home, LogName::Plan).path().to_path_buf();
    let kinds = ["analysis", "estimate", "verdict"];
    assert_eq!(after_the_rows(&lab, "once"), kinds);
    let all = common::records_of(&lab, "once");
    let n = all.len();
    let a = all.iter().find_map(Analysis::of).unwrap();
    assert_eq!((a.estimates, a.verdicts, a.slot), (1, 1, Some(1)));
    let closed = stream_of(&lab, LogName::Plan);
    let kinds_of = |s: &[(String, serde_json::Value)]| -> Vec<String> {
        s.iter().map(|(k, _)| k.clone()).collect()
    };
    assert_eq!(
        kinds_of(&closed),
        ["stream.config", "stream.open", "stream.close"]
    );
    let bodies = |rs: &[records::Record]| -> Vec<serde_json::Value> {
        rs.iter().map(|r| r.body.clone()).collect()
    };
    // A second run adds nothing, to the study's log or to the stream.
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    assert_eq!(common::records_of(&lab, "once").len(), n);
    assert_eq!(stream_of(&lab, LogName::Plan), closed);
    // A crash before the close: after the verdict, after the estimate, after the analysis
    // record, or before it. The next run adds what is missing, once, and closes the slot
    // with the same e-value.
    for kept in [n, n - 1, n - 2, n - 3] {
        keep_records(&dir, kept);
        keep_lines(&stream, 2);
        assert!(!finished(&lab, "once"), "kept {kept}");
        assert_eq!(run(&plan).unwrap(), RunStatus::Done);
        assert_eq!(after_the_rows(&lab, "once"), kinds, "kept {kept}");
        let now = common::records_of(&lab, "once");
        assert_eq!(bodies(&now[n - 3..]), bodies(&all[n - 3..]), "kept {kept}");
        assert_eq!(stream_of(&lab, LogName::Plan), closed, "kept {kept}");
        assert!(finished(&lab, "once"), "kept {kept}");
    }
    // Another box version wrote every record but the close, and its verdict holds an e-value
    // of 250: the rerun adds no record and closes the slot with the e-value the signed
    // verdicts give, not one of its own.
    keep_records(&dir, n - 3);
    let mut old = all[n - 3].body.clone();
    old["box_version"] = serde_json::json!("0.0.0");
    append_record(&lab, "once", "analysis", old);
    append_record(&lab, "once", "estimate", all[n - 2].body.clone());
    let mut verdict = all[n - 1].body.clone();
    verdict["e_value"] = serde_json::json!(250.0);
    append_record(&lab, "once", "verdict", verdict);
    keep_lines(&stream, 2);
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    assert_eq!(common::records_of(&lab, "once").len(), n);
    let stream = stream_of(&lab, LogName::Plan);
    assert_eq!(stream.last(), Some(&close_of("once", 250.0, true)));
    assert!(finished(&lab, "once"));
}
