//! The card's sections, built from a finished confirmatory study with two strata.
mod common;
use common::{Lab, estimate, finished_with, instrument, lab, plant};
use ignorarium::canon::sha256_hex;
use ignorarium::card::sections::{Field, NOT_IN_RECORDS, Outside};
use ignorarium::card::{self, Card};
use ignorarium::derived::GatePrediction;
use ignorarium::rules::{Curve, CurvePoint};
use ignorarium::{keys, records, spec};
use serde_json::{Value, json};

const ID: &str = "s1";
/// ln(1.04): the claim's margin, +4%.
const MARGIN: f64 = 0.039221;
/// The estimate in each stratum: exp(0.0287) - 1 = 2.91%.
const ESTIMATE: f64 = 0.0287;
const LAUNCHER: &str = "cloud-hypervisor v41.0";

fn strata() -> Vec<String> {
    vec!["code".into(), "article".into()]
}

/// The plan: the template measured in both strata, made a confirmatory claim below `MARGIN`
/// on `statistic`, held in both strata, on a log-ratio outcome, with a memory cap.
fn spec_text(statistic: &str) -> String {
    common::source_spec(&strata())
        .replace("mode = \"exploratory\"", "mode = \"confirmatory\"")
        .replace("unit = \"ratio\"", "unit = \"log ratio\"")
        .replace("paired = \"percent\"", "paired = \"log-ratio\"")
        .replace("strata = []", "strata = [\"code\", \"article\"]")
        .replace(
            "# claim = { below = 0.0 }",
            &format!("claim = {{ below = {MARGIN} }}"),
        )
        .replace(
            "# statistic = { quantile = 0.5 }",
            &format!("statistic = {statistic}"),
        )
        .replace("# row_memory_mib = 1024", "row_memory_mib = 2304")
}

const MEAN: &str = "{ mean = [-0.693147, 0.693147] }";

/// The gate's curve from `power_at` 0.0198 to the margin.
fn curve() -> Curve {
    let at = |fraction: f64, power: f64| CurvePoint {
        fraction,
        mean: 0.0198 + fraction * (MARGIN - 0.0198),
        power,
    };
    Curve {
        margin: MARGIN,
        points: vec![
            at(0.0, 0.98),
            at(0.25, 0.81),
            at(0.5, 0.33),
            at(0.75, 0.04),
            at(1.0, 0.0),
        ],
    }
}

/// A finished study `ID` on plan `spec_text(statistic)`: estimates of `ESTIMATE` in each
/// stratum and pooled, one `fail` verdict, and rows whose `arm-a` values pair to `ESTIMATE`.
/// `row_keys` are added to every row record.
fn fixture(lab: &Lab, statistic: &str, row_keys: Value) {
    let text = spec_text(statistic);
    let arm = [("arm-a", 1.0, json!({}))];
    let estimates = ["code", "article", ""]
        .iter()
        .map(|s| {
            let mut e = estimate(&arm, s, 4.0, Some(0.1));
            e["mean"] = json!(ESTIMATE);
            e
        })
        .collect();
    let mut bodies = finished_with(instrument(), None, estimates);
    for (kind, body) in bodies.iter_mut() {
        match *kind {
            "register" => {
                body["spec_hash"] = json!(spec::parse(text.as_bytes()).unwrap().hash);
                body["gate"] = json!([GatePrediction {
                    hypothesis: "question.hypotheses[0]".into(),
                    id: "H1".into(),
                    power: 0.98,
                    target: 0.8,
                    level: 0.005,
                    alpha_share: 1.0,
                    reps: 1000,
                    power_at: 0.0198,
                    margins: vec![MARGIN],
                    resized: false,
                    needed: None,
                    groups: vec![],
                    curve: vec![curve()],
                }]);
            }
            "analysis" => body["verdicts"] = json!(1),
            "row" => {
                if body["condition"] == "arm-a" {
                    body["values"]["ppl"] = json!(10.0 * ESTIMATE.exp());
                }
                for (k, v) in row_keys.as_object().unwrap() {
                    body[k] = v.clone();
                }
            }
            _ => {}
        }
    }
    let bound = |s: &str| json!({ "stratum": s, "side": "below", "margin": MARGIN, "clusters": 4, "value": 3.1 });
    bodies.push((
        "verdict",
        json!({
            "hypothesis": "H1", "verdict": "fail", "basis": "test", "level": 0.005,
            "e_value": 3.1, "bounds": [bound("code"), bound("article")],
            "incomplete_units": 0, "reason": null,
        }),
    ));
    std::fs::create_dir_all(lab.home.study_dir(ID)).unwrap();
    std::fs::write(lab.home.study_dir(ID).join("spec.toml"), text).unwrap();
    plant(&lab.home, ID, &bodies);
}

fn rows_on_level_2() -> Value {
    json!({ "isolation": 2, "launcher": LAUNCHER, "agent_sha256": "ab".repeat(32) })
}

fn card_of(lab: &Lab) -> Card {
    let key = keys::load(&lab.home).unwrap().unwrap();
    let path = lab.home.study_dir(ID).join("records.jsonl");
    let log = records::read_log(&path, &key.verifying_key()).unwrap();
    card::build(&lab.home, ID, &log).unwrap_or_else(|e| panic!("{}", e.msg))
}

fn recorded<T>(value: T) -> Field<T> {
    Field::Recorded { value }
}

#[test]
fn stack_from_row_records() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let stack = card_of(&lab).setup.stack;
    assert_eq!(stack.isolation, recorded(vec![2]));
    assert_eq!(stack.launcher, recorded(vec![LAUNCHER.to_string()]));
    assert_eq!(stack.agent_sha256, recorded(vec!["ab".repeat(32)]));
    assert_eq!(stack.memory_cap_mib, recorded(2304));
}

#[test]
fn display_word_not_shown_for_fail() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let lines = card_of(&lab).verdicts;
    assert_eq!(lines.len(), 1);
    assert_eq!(
        (lines[0].hypothesis.as_str(), lines[0].display.as_str()),
        ("H1", "not shown")
    );
    let word = card::sections::display_word;
    assert_eq!(
        [word("pass"), word("fail"), word("none")],
        ["shown", "not shown", "no claim"]
    );
}

#[test]
fn label_not_computed() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    assert_eq!(card_of(&lab).verdicts[0].label, "label not computed");
}

#[test]
fn percent_change_for_log_ratio() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let results = card_of(&lab).results;
    let code = results.iter().find(|r| r.stratum == "code").unwrap();
    assert_eq!(code.estimate, ESTIMATE);
    let change = code.percent_change.unwrap();
    assert_eq!(change, ESTIMATE.exp() - 1.0);
    assert_eq!(format!("{:.2}%", change * 100.0), "2.91%");
    assert_eq!(
        (code.e_value, code.clusters, code.units),
        (Some(3.1), 4, 20)
    );
    assert_eq!(code.intervals.len(), 1);
    assert!(code.registered);
    // Another outcome unit has no percent change.
    let other = lab_with_percent_outcome();
    assert!(other.results.iter().all(|r| r.percent_change.is_none()));
}

fn lab_with_percent_outcome() -> Card {
    let lab = lab();
    let bodies = finished_with(instrument(), None, vec![common::arm_a()]);
    plant(&lab.home, "s1", &bodies);
    card_of(&lab)
}

#[test]
fn quantile_claim_states_no_interval() {
    let lab = lab();
    fixture(&lab, "{ quantile = 0.5 }", rows_on_level_2());
    let card = card_of(&lab);
    let code = card.results.iter().find(|r| r.stratum == "code").unwrap();
    assert!(code.intervals.is_empty());
    assert_eq!(
        code.note.as_deref(),
        Some("the interval is not built for quantile claims")
    );
    let mean = {
        let lab = self::lab();
        fixture(&lab, MEAN, rows_on_level_2());
        card_of(&lab)
    };
    assert!(mean.results.iter().all(|r| r.note.is_none()));
}

#[test]
fn sensitivity_reads_gate_curve() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let s = card_of(&lab).sensitivity;
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].hypothesis, "H1");
    assert_eq!(s[0].curve, vec![curve()]);
    // 0.0287 lies between the points at 0.25 (0.0247...) and 0.5 (0.0295...).
    let code = s[0].placed.iter().find(|p| p.stratum == "code").unwrap();
    let points = &curve().points;
    assert_eq!(code.estimate, ESTIMATE);
    assert_eq!(code.margin, MARGIN);
    assert_eq!(code.lower.as_ref(), Some(&points[1]));
    assert_eq!(code.upper.as_ref(), Some(&points[2]));
    assert_eq!(s[0].placed.len(), 2, "the claim's strata only");
}

#[test]
fn missing_field_prints_not_in_records() {
    let lab = lab();
    fixture(&lab, MEAN, json!({ "isolation": 2, "launcher": LAUNCHER }));
    let events = lab.home.study_dir(ID).join("events.jsonl");
    std::fs::write(&events, "{\"name\":\"run.start\"}\n").unwrap();
    let card = card_of(&lab);
    let stack = &card.setup.stack;
    let link = Outside {
        path: "events.jsonl".into(),
        sha256: sha256_hex(&std::fs::read(&events).unwrap()),
    };
    assert_eq!(stack.agent_sha256, Field::Missing { link: Some(link) });
    assert_eq!(stack.agent_sha256.shown(|v| v.join(", ")), NOT_IN_RECORDS);
    assert_eq!(NOT_IN_RECORDS, "not in the records");
    assert_eq!(stack.vcpus, Field::Missing { link: None });
    assert_eq!(stack.vcpus.shown(|v| v.to_string()), NOT_IN_RECORDS);
    assert_eq!(stack.launcher.shown(|v| v.join(", ")), LAUNCHER);
    let json = serde_json::to_value(&card).unwrap();
    assert_eq!(
        json["setup"]["stack"]["vcpus"],
        json!({ "state": "missing", "link": null })
    );
}

#[test]
fn outside_files_hashed_not_parsed() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let dir = lab.home.study_dir(ID);
    let said = "e-value 987654 shown";
    let files = ["statement.md", "gaps.md", "decisions.md", "deviations.toml"];
    for name in files {
        let text = if name == "deviations.toml" {
            String::new()
        } else {
            format!("# {name}\n{said}\n")
        };
        std::fs::write(dir.join(name), text).unwrap();
    }
    std::fs::write(dir.join("readings.md"), format!("## TL;DR\n{said}\n")).unwrap();
    let card = card_of(&lab);
    let want: Vec<Outside> = (files.iter())
        .map(|name| Outside {
            path: name.to_string(),
            sha256: sha256_hex(&std::fs::read(dir.join(name)).unwrap()),
        })
        .collect();
    assert_eq!(card.outside.files, want);
    assert_eq!(card.outside.readings, "readings.md");
    let json = serde_json::to_string(&card).unwrap();
    assert!(!json.contains("987654"), "an outside file was parsed");
    let readings = sha256_hex(&std::fs::read(dir.join("readings.md")).unwrap());
    assert!(
        !json.contains(&readings),
        "readings.md is named by path only"
    );
}
