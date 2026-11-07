//! `report`: the verdicts of a finished study, through the real binary.
mod common;
use assert_cmd::Command;
use common::{Lab, finished_with, instrument, lab, plant, records_of};
use serde_json::{Value, json};

fn bin(lab: &Lab) -> Command {
    let mut c = Command::cargo_bin("ignorarium").unwrap();
    c.env("IGNORARIUM_HOME", &lab.home.root);
    c
}

fn text(a: &assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&a.get_output().stdout).into_owned()
}

fn verdict(h: &str, v: &str, basis: &str, e: Value, level: Value, reason: Value) -> Value {
    json!({
        "hypothesis": h, "verdict": v, "basis": basis, "level": level, "e_value": e,
        "bounds": [], "incomplete_units": 0, "reason": reason,
    })
}

fn clip(h: &str, stratum: &str, below: u64, above: u64, units: u64) -> Value {
    json!({ "hypothesis": h, "stratum": stratum, "below": below, "above": above, "units": units })
}

/// A finished study whose analysis promises `verdicts` and carries `clipped`.
fn finished_study(verdicts: &[Value], clipped: &[Value]) -> Vec<(&'static str, Value)> {
    let mut bodies = finished_with(instrument(), None, vec![]);
    let analysis = bodies.iter_mut().find(|(k, _)| *k == "analysis").unwrap();
    analysis.1["verdicts"] = json!(verdicts.len());
    analysis.1["clipped"] = json!(clipped);
    bodies.extend(verdicts.iter().map(|v| ("verdict", v.clone())));
    bodies
}

fn three() -> Vec<Value> {
    vec![
        verdict(
            "h1",
            "pass",
            "test",
            json!(312.4),
            json!(0.005),
            Value::Null,
        ),
        verdict(
            "h2",
            "fail",
            "test",
            json!(0.0),
            json!(0.005),
            json!("incomplete units in scope: 2"),
        ),
        verdict("h3", "none", "none", Value::Null, Value::Null, Value::Null),
    ]
}

fn report_json(lab: &Lab, id: &str) -> Value {
    let a = bin(lab).args(["report", id, "--json"]).assert().success();
    serde_json::from_str(&text(&a)).unwrap()
}

#[test]
fn a_finished_study_reports_each_verdict() {
    let lab = lab();
    let bodies = finished_study(&three(), &[]);
    plant(&lab.home, "s1", &bodies);
    let r = report_json(&lab, "s1");
    let last = records_of(&lab, "s1")
        .iter()
        .rev()
        .find(|x| x.kind == "verdict")
        .map(|x| x.ts.clone())
        .unwrap();
    assert_eq!(r["study"], "s1");
    assert_eq!(r["finished_at"], last);
    assert_eq!(r["records"], bodies.len());
    let want = json!([
        { "hypothesis": "h1", "verdict": "pass", "basis": "test", "e_value": 312.4,
          "level": 0.005, "reason": null, "clipped": null },
        { "hypothesis": "h2", "verdict": "fail", "basis": "test", "e_value": 0.0,
          "level": 0.005, "reason": "incomplete units in scope: 2", "clipped": null },
        { "hypothesis": "h3", "verdict": "none", "basis": "none", "e_value": null,
          "level": null, "reason": null, "clipped": null },
    ]);
    assert_eq!(r["hypotheses"], want);
    let opened = records_of(&lab, "s1").pop().unwrap();
    assert_eq!(opened.kind, "opened");
    assert_eq!(r["opened"], json!({ "seq": opened.seq, "ts": opened.ts }));
}

#[test]
fn text_and_json_carry_the_same_facts() {
    let lab = lab();
    let bodies = finished_study(&three(), &[]);
    plant(&lab.home, "s1", &bodies);
    let a = bin(&lab).args(["report", "s1"]).assert().success();
    let t = text(&a);
    for line in [
        "h1: pass (test), e-value 312.40, level 0.0050\n",
        "h2: fail (test), e-value 0.00, level 0.0050: incomplete units in scope: 2\n",
        "h3: none (none)\n",
        &format!(
            "study s1: finished {}, {} records verified\n",
            ts_of(&lab),
            bodies.len()
        ),
    ] {
        assert!(t.contains(line), "missing {line:?} in {t}");
    }
}

fn ts_of(lab: &Lab) -> String {
    records_of(lab, "s1")
        .iter()
        .rev()
        .find(|x| x.kind == "verdict")
        .map(|x| x.ts.clone())
        .unwrap()
}

#[test]
fn a_mean_claim_prints_its_clip_counts_per_group() {
    let lab = lab();
    let clips = [clip("h1", "code", 1, 2, 40), clip("h1", "", 3, 2, 120)];
    plant(&lab.home, "s1", &finished_study(&three(), &clips));
    let r = report_json(&lab, "s1");
    assert_eq!(
        r["hypotheses"][0]["clipped"],
        json!([
            { "stratum": "code", "below": 1, "above": 2, "units": 40 },
            { "stratum": "", "below": 3, "above": 2, "units": 120 },
        ])
    );
    let a = bin(&lab).args(["report", "s1"]).assert().success();
    let t = text(&a);
    let want = "  clipped in code: 1 below lo, 2 above hi, of 40 units\n  clipped in all: 3 below lo, 2 above hi, of 120 units\n";
    assert!(t.contains(want), "{t}");
}

#[test]
fn a_hypothesis_without_counts_prints_none() {
    let lab = lab();
    let clips = [clip("h1", "", 3, 2, 120)];
    plant(&lab.home, "s1", &finished_study(&three(), &clips));
    let r = report_json(&lab, "s1");
    assert_eq!(r["hypotheses"][1]["clipped"], Value::Null);
    let a = bin(&lab).args(["report", "s1"]).assert().success();
    let t = text(&a);
    let h2 = t.split("h2:").nth(1).unwrap();
    let h2 = h2.split("h3:").next().unwrap();
    assert!(!h2.contains("clipped"), "{t}");
}
