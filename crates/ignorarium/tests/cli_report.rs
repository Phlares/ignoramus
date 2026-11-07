//! `report`: the verdicts of a finished study, through the real binary.
mod common;
use assert_cmd::Command;
use common::{Lab, edited, finished_with, instrument, lab, plain, plant, records_of, source_spec};
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

fn log_path(lab: &Lab, id: &str) -> std::path::PathBuf {
    lab.home.study_dir(id).join("records.jsonl")
}

fn refused(lab: &Lab, id: &str, said: &[&str]) {
    let a = bin(lab).args(["report", id]).assert().code(1);
    let err = String::from_utf8_lossy(&a.get_output().stderr).into_owned();
    for s in said {
        assert!(err.contains(s), "missing {s:?} in {err}");
    }
}

fn old_analysis_body() -> Value {
    json!({
        "commit": "ef".repeat(32), "box_version": "0.0.1", "estimates": 0, "verdicts": 1,
        "slot": null, "instrument": { "payload": "", "environment": "", "inputs": [] },
        "recheck": null,
    })
}

#[test]
fn each_report_appends_one_opened_record() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let first = report_json(&lab, "s1");
    let second = report_json(&lab, "s1");
    let all = records_of(&lab, "s1");
    let tail = &all[all.len() - 2..];
    for (r, shown) in tail.iter().zip([&first, &second]) {
        assert_eq!(r.kind, "opened");
        assert_eq!(shown["opened"]["seq"], r.seq);
    }
    bin(&lab)
        .args(["verify", "--study", "s1"])
        .assert()
        .success();
    assert_eq!(first["records"].as_u64().unwrap() + 1, second["records"]);
}

#[test]
fn a_tampered_record_refuses_the_report() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let path = log_path(&lab, "s1");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"e_value\":312.4"));
    std::fs::write(
        &path,
        text.replacen("\"e_value\":312.4", "\"e_value\":312.5", 1),
    )
    .unwrap();
    let before = std::fs::read(&path).unwrap();
    refused(
        &lab,
        "s1",
        &["cannot report s1: record ", "hash does not match"],
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn a_cut_last_line_refuses_the_report() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let path = log_path(&lab, "s1");
    let mut bytes = std::fs::read(&path).unwrap();
    let lines = bytes.iter().filter(|b| **b == b'\n').count();
    bytes.truncate(bytes.len() - 1 - 10);
    std::fs::write(&path, &bytes).unwrap();
    refused(
        &lab,
        "s1",
        &[&format!("line {lines} is not a whole record")],
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn an_unfinished_study_refuses_blind_or_not() {
    let lab = lab();
    for (id, blind) in [("u1", false), ("u2", true)] {
        if blind {
            edited(
                &lab,
                id,
                |t| t.replace("blind = false", "blind = true"),
                common::PAYLOAD,
            );
        } else {
            plain(&lab, id);
        }
        let before = std::fs::read(log_path(&lab, id)).unwrap();
        refused(
            &lab,
            id,
            &[&format!(
                "study {id} is not finished; status shows its progress"
            )],
        );
        assert_eq!(std::fs::read(log_path(&lab, id)).unwrap(), before);
    }
}

#[test]
fn a_blind_finished_study_shows_everything() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let dir = lab.home.study_dir("s2");
    std::fs::create_dir_all(&dir).unwrap();
    let blind = source_spec(&[]).replace("blind = false", "blind = true");
    assert!(blind.contains("blind = true"));
    std::fs::write(dir.join("spec.toml"), blind).unwrap();
    plant(&lab.home, "s2", &finished_study(&three(), &[]));
    let open = report_json(&lab, "s1");
    let shut = report_json(&lab, "s2");
    assert_eq!(open["hypotheses"], shut["hypotheses"]);
    assert_eq!(open["records"], shut["records"]);
}

#[test]
fn a_stopped_study_refuses_with_its_reason() {
    let lab = lab();
    plain(&lab, "s1");
    plant(&lab.home, "s1", &[("stop", json!({ "why": "budget" }))]);
    refused(
        &lab,
        "s1",
        &["study s1 is stopped: budget; it has no report"],
    );
}

#[test]
fn an_old_analysis_record_reports_without_counts() {
    let lab = lab();
    let mut bodies = finished_study(&three()[..1], &[]);
    bodies.iter_mut().find(|(k, _)| *k == "analysis").unwrap().1 = old_analysis_body();
    plant(&lab.home, "s1", &bodies);
    let r = report_json(&lab, "s1");
    assert_eq!(r["hypotheses"][0]["clipped"], Value::Null);
    let a = bin(&lab).args(["report", "s1"]).assert().success();
    assert!(!text(&a).contains("clipped"));
}

#[test]
fn no_study_and_not_registered_refuse() {
    let lab = lab();
    refused(&lab, "Bad_Id", &["is not a study id"]);
    refused(&lab, "ghost", &["is not registered"]);
}

#[test]
fn a_second_verdict_for_a_hypothesis_is_not_printed() {
    let lab = lab();
    let first = verdict(
        "h1",
        "pass",
        "test",
        json!(312.4),
        json!(0.005),
        Value::Null,
    );
    let second = verdict("h1", "fail", "test", json!(0.0), json!(0.005), Value::Null);
    let mut bodies = finished_study(&[first], &[]);
    bodies.push(("verdict", second));
    plant(&lab.home, "s1", &bodies);
    let r = report_json(&lab, "s1");
    let shown = r["hypotheses"].as_array().unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0]["verdict"], "pass");
    assert_eq!(shown[0]["e_value"], 312.4);
}

#[test]
fn a_finished_study_with_a_later_stop_record_still_reports() {
    let lab = lab();
    let mut bodies = finished_study(&three(), &[]);
    bodies.push(("stop", json!({ "why": "budget" })));
    plant(&lab.home, "s1", &bodies);
    let r = report_json(&lab, "s1");
    assert_eq!(r["hypotheses"].as_array().unwrap().len(), 3);
    assert_eq!(r["hypotheses"][0]["verdict"], "pass");
    assert_eq!(records_of(&lab, "s1").pop().unwrap().kind, "opened");
}
