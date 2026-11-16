mod common;

use ignorarium::derived::{By, DoneRecord, End};
use ignorarium::done::end_of;
use ignorarium::records::{self, Record};
use serde_json::{Value, json};

/// `bodies` as the records of study `d`, record `i` written at second `i` of the day.
fn log(bodies: &[(&str, Value)]) -> Vec<Record> {
    let mut all = common::records_for("d", bodies);
    for r in &mut all {
        r.ts = format!("2026-01-01T00:00:{:02}Z", r.seq);
    }
    all
}

/// The done body that `by`, record `seq` of `all`, gives with `end` at `ended_at`.
fn done(all: &[Record], end: End, ended_at: &str, seq: usize) -> Option<DoneRecord> {
    Some(DoneRecord {
        end,
        ended_at: ended_at.into(),
        by: By {
            seq: seq as u64,
            hash: records::hash(&all[seq]),
        },
    })
}

/// A finished study: register 0, analysis 1, estimate 2, then its rows.
fn finished() -> Vec<(&'static str, Value)> {
    common::finished(vec![common::arm_a()])
}

fn stop() -> (&'static str, Value) {
    ("stop", json!({ "why": "manual" }))
}

#[test]
fn a_finished_study_is_done_by_its_rows() {
    let all = log(&finished());
    assert_eq!(end_of(&all, None), done(&all, End::Rows, &all[2].ts, 2));
}

#[test]
fn the_last_promised_record_ends_the_study() {
    let mut bodies = finished();
    bodies[1].1["verdicts"] = json!(1);
    let verdict = json!({
        "hypothesis": "H1", "verdict": "none", "basis": "none", "level": null,
        "e_value": null, "bounds": [], "incomplete_units": 0, "reason": null,
    });
    bodies.insert(3, ("verdict", verdict));
    let all = log(&bodies);
    assert_eq!(end_of(&all, None), done(&all, End::Rows, &all[3].ts, 3));
}

#[test]
fn a_repeated_estimate_does_not_move_the_record_that_ends_it() {
    let mut bodies = finished();
    bodies.push(("estimate", common::arm_a()));
    let all = log(&bodies);
    assert_eq!(end_of(&all, None), done(&all, End::Rows, &all[2].ts, 2));
}

#[test]
fn an_analysis_that_promises_nothing_ends_the_study_itself() {
    let mut bodies = finished();
    bodies.remove(2);
    bodies[1].1["estimates"] = json!(0);
    let all = log(&bodies);
    assert_eq!(end_of(&all, None), done(&all, End::Rows, &all[1].ts, 1));
}

#[test]
fn an_estimate_before_the_analysis_does_not_end_the_study() {
    let mut bodies = finished();
    bodies.swap(1, 2);
    bodies[2].1["estimates"] = json!(0);
    let all = log(&bodies);
    assert_eq!(all[1].kind, "estimate");
    assert_eq!(end_of(&all, None), done(&all, End::Rows, &all[2].ts, 2));
}

#[test]
fn a_resize_that_stops_ends_as_a_resize_stop() {
    let resize = |decision: &str| {
        let reason = (decision != "continue").then_some("short");
        json!({ "strata": [], "decision": decision, "reason": reason })
    };
    for (decision, end) in [("stop", End::ResizeStop), ("continue", End::Rows)] {
        let mut bodies = finished();
        bodies.insert(1, ("resize", resize(decision)));
        let all = log(&bodies);
        assert_eq!(
            end_of(&all, None),
            done(&all, end, &all[3].ts, 3),
            "{decision}"
        );
    }
}

#[test]
fn a_stop_record_alone_ends_the_study_as_a_stop() {
    let mut bodies = finished();
    bodies.truncate(1);
    bodies.push(stop());
    let all = log(&bodies);
    assert_eq!(end_of(&all, None), done(&all, End::Stop, &all[1].ts, 1));
}

#[test]
fn of_a_finish_and_a_stop_the_one_first_in_the_log_ends_the_study() {
    let mut bodies = finished();
    bodies.push(stop());
    let all = log(&bodies);
    assert_eq!(end_of(&all, None), done(&all, End::Rows, &all[2].ts, 2));
    // A stop before the analysis came first.
    let mut bodies = finished();
    bodies.insert(1, stop());
    let all = log(&bodies);
    assert_eq!(end_of(&all, None), done(&all, End::Stop, &all[1].ts, 1));
}

#[test]
fn a_study_neither_finished_nor_stopped_has_no_end() {
    let mut bodies = finished();
    bodies.remove(2);
    assert_eq!(end_of(&log(&bodies), None), None);
    bodies.truncate(1);
    assert_eq!(end_of(&log(&bodies), None), None);
}

#[test]
fn a_held_slot_ends_the_study_once_closed_at_the_close_time() {
    let mut bodies = finished();
    bodies[1].1["slot"] = json!(0);
    let all = log(&bodies);
    assert_eq!(end_of(&all, None), None);
    let close = "2026-01-02T00:00:00Z";
    assert_eq!(end_of(&all, Some(close)), done(&all, End::Rows, close, 2));
    // A close before the last estimate leaves the estimate's time.
    let early = "2025-12-31T00:00:00Z";
    assert_eq!(
        end_of(&all, Some(early)),
        done(&all, End::Rows, &all[2].ts, 2)
    );
}

#[test]
fn the_done_body_reads_back_and_refuses_another_field() {
    let all = log(&finished());
    let body = serde_json::to_value(end_of(&all, None).unwrap()).unwrap();
    assert_eq!(
        body,
        json!({ "end": "rows", "ended_at": all[2].ts, "by": { "seq": 2, "hash": records::hash(&all[2]) } })
    );
    let mut r = all[0].clone();
    r.kind = "done".into();
    r.body = body.clone();
    assert!(DoneRecord::of(&r).is_some());
    r.body["extra"] = json!(1);
    assert_eq!(DoneRecord::of(&r), None);
    r.body = body;
    r.kind = "stop".into();
    assert_eq!(DoneRecord::of(&r), None);
    for (end, text) in [
        (End::Rows, "rows"),
        (End::ResizeStop, "resize-stop"),
        (End::Stop, "stop"),
    ] {
        assert_eq!(serde_json::to_value(end).unwrap(), json!(text));
    }
}
