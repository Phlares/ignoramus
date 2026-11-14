//! `deviations.toml`: the unsigned deviation record, and its place on the card.
mod common;
use common::{bin, finished_with, instrument, lab, plant, text};
use ignorarium::control::{self, DeviationRow, Timing};
use ignorarium::deviations;
use serde_json::{Value, json};
use std::path::Path;

const FILE: &str = r#"
[[deviation]]
kind = "analysis"
reason = "the payload crashed on one seed"
timing = "after-access-before-results"
original = "five seeds"
justification = "one seed hung"
impact = "four seeds reach the test"
source = "notes.md:12"

[[unregistered]]
step = "reran the extractor"
reason = "disk full"
timing = "during-collection"
source = "sha256:ab"
"#;

fn write(dir: &Path, text: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("deviations.toml"), text).unwrap();
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ign-dev-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn loads_willroth_fields() {
    let dir = scratch("loads");
    write(&dir, FILE);
    let d = deviations::load(&dir).unwrap().unwrap();
    assert_eq!(d.deviation.len(), 1);
    assert_eq!(d.deviation[0].row.kind, "analysis");
    assert_eq!(d.deviation[0].row.timing, Timing::AfterAccessBeforeResults);
    assert_eq!(d.deviation[0].source, "notes.md:12");
    assert_eq!(d.unregistered[0].step, "reran the extractor");
    assert_eq!(d.unregistered[0].timing, Timing::DuringCollection);
    assert_eq!(d.sha256, ignorarium::canon::sha256_hex(FILE.as_bytes()));
}

#[test]
fn unknown_timing_refused() {
    let dir = scratch("timing");
    write(&dir, &FILE.replace("during-collection", "whenever"));
    let e = deviations::load(&dir).unwrap_err();
    assert!(e.contains("whenever"), "{e}");
    for stage in [
        "before-collection",
        "during-collection",
        "after-collection-before-access",
        "after-access-before-results",
        "after-results",
    ] {
        assert!(e.contains(stage), "missing {stage} in {e}");
    }
}

#[test]
fn unknown_field_refused_by_name() {
    let dir = scratch("field");
    write(&dir, &FILE.replace("step =", "stepp ="));
    let e = deviations::load(&dir).unwrap_err();
    assert!(e.contains("stepp"), "{e}");
}

#[test]
fn missing_file_is_none() {
    assert_eq!(deviations::load(&scratch("none")).unwrap(), None);
}

fn signed_study() -> Vec<(&'static str, Value)> {
    let mut bodies = finished_with(instrument(), None, vec![]);
    let register = bodies.iter_mut().find(|(k, _)| *k == "register").unwrap();
    register.1["deviations"] = json!([{
        "differs": ["payload"], "from": "pilot", "reason": "new payload",
        "timing": "before-collection",
    }]);
    bodies
}

#[test]
fn card_merges_signed_and_file_rows() {
    let lab = lab();
    plant(&lab.home, "s1", &signed_study());
    write(&lab.home.study_dir("s1"), FILE);
    let a = bin(&lab)
        .args(["report", "s1", "--json"])
        .assert()
        .success();
    let r: Value = serde_json::from_str(&text(&a)).unwrap();
    let hash = ignorarium::canon::sha256_hex(FILE.as_bytes());
    assert_eq!(
        r["deviations"],
        json!([
            { "mark": "signed", "from": "pilot", "reason": "new payload",
              "timing": "before-collection", "differs": ["payload"] },
            { "mark": "unsigned", "entry": "deviation", "subject": "analysis",
              "reason": "the payload crashed on one seed",
              "timing": "after-access-before-results", "original": "five seeds",
              "justification": "one seed hung",
              "impact": "four seeds reach the test", "source": "notes.md:12", "sha256": hash },
            { "mark": "unsigned", "entry": "unregistered", "subject": "reran the extractor",
              "reason": "disk full", "timing": "during-collection", "original": null,
              "justification": null, "impact": null, "source": "sha256:ab", "sha256": hash },
        ])
    );
}

#[test]
fn card_without_a_file_has_only_signed_rows() {
    let lab = lab();
    plant(&lab.home, "s1", &signed_study());
    let a = bin(&lab)
        .args(["report", "s1", "--json"])
        .assert()
        .success();
    let r: Value = serde_json::from_str(&text(&a)).unwrap();
    assert_eq!(r["deviations"].as_array().unwrap().len(), 1);
    assert_eq!(r["deviations"][0]["mark"], "signed");
}

#[test]
fn a_malformed_file_refuses_the_report_naming_the_file() {
    let lab = lab();
    plant(&lab.home, "s1", &signed_study());
    write(&lab.home.study_dir("s1"), "[[deviation]\n");
    let a = bin(&lab).args(["report", "s1"]).assert().code(2);
    let err = String::from_utf8_lossy(&a.get_output().stderr).into_owned();
    assert!(err.contains("deviations.toml"), "{err}");
}

#[test]
fn a_deviation_without_a_source_is_refused_by_name() {
    let dir = scratch("source");
    write(
        &dir,
        &FILE.replace(
            "source = \"notes.md:12\"
",
            "",
        ),
    );
    let e = deviations::load(&dir).unwrap_err();
    assert!(e.contains("source"), "{e}");
}

#[test]
fn a_deviation_without_a_justification_is_refused_by_name() {
    let dir = scratch("just");
    write(
        &dir,
        &FILE.replace(
            "justification = \"one seed hung\"
",
            "",
        ),
    );
    let e = deviations::load(&dir).unwrap_err();
    assert!(e.contains("justification"), "{e}");
}

fn recorded_row() -> DeviationRow {
    DeviationRow {
        kind: "stop".into(),
        reason: "disk".into(),
        timing: Timing::DuringCollection,
        original: "run all rows".into(),
        justification: "disk full".into(),
        impact: "fewer rows".into(),
    }
}

#[test]
fn card_includes_recorded_rows() {
    let lab = lab();
    plant(&lab.home, "s1", &signed_study());
    control::deviation(&lab.home, "s1", &recorded_row()).unwrap();
    write(&lab.home.study_dir("s1"), FILE);
    let a = bin(&lab)
        .args(["report", "s1", "--json"])
        .assert()
        .success();
    let r: Value = serde_json::from_str(&text(&a)).unwrap();
    let marks: Vec<&str> = (r["deviations"].as_array().unwrap().iter())
        .map(|d| d["mark"].as_str().unwrap())
        .collect();
    assert_eq!(marks, ["signed", "recorded", "unsigned", "unsigned"]);
    let bytes = std::fs::read(lab.home.study_dir("s1").join("deviations.jsonl")).unwrap();
    assert_eq!(
        r["deviations"][1],
        json!({
            "mark": "recorded", "kind": "stop", "reason": "disk", "timing": "during-collection",
            "original": "run all rows", "justification": "disk full", "impact": "fewer rows",
            "sha256": ignorarium::canon::sha256_hex(&bytes),
        })
    );
}

#[test]
fn a_malformed_recorded_file_refuses_the_report_naming_the_file() {
    let lab = lab();
    plant(&lab.home, "s1", &signed_study());
    std::fs::write(
        lab.home.study_dir("s1").join("deviations.jsonl"),
        "{nope
",
    )
    .unwrap();
    let a = bin(&lab).args(["report", "s1"]).assert().code(2);
    let err = String::from_utf8_lossy(&a.get_output().stderr).into_owned();
    assert!(err.contains("deviations.jsonl"), "{err}");
}
