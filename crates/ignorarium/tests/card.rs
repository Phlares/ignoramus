//! The card's sections, built from a finished confirmatory study with two strata.
mod common;
use common::{Lab, estimate, finished_with, instrument, lab, plant};
use ignorarium::canon::sha256_hex;
use ignorarium::card::sections::{
    Attempt, Field, Listed, NOT_APPLICABLE, NOT_IN_RECORDS, Outside, ROW_SEED, UNRECORDED,
};
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
    plant(&lab.home, ID, &bodies(lab, statistic, row_keys));
}

/// The records `fixture` plants, with the plan already stored.
fn bodies(lab: &Lab, statistic: &str, row_keys: Value) -> Vec<(&'static str, Value)> {
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
    bodies
}

fn rows_on_level_2() -> Value {
    json!({ "isolation": 2, "launcher": LAUNCHER, "agent_sha256": "ab".repeat(32) })
}

fn card_of(lab: &Lab) -> Card {
    card_of_study(lab, ID)
}

fn card_of_study(lab: &Lab, id: &str) -> Card {
    let key = keys::load(&lab.home).unwrap().unwrap();
    let path = lab.home.study_dir(id).join("records.jsonl");
    let log = records::read_log(&path, &key.verifying_key()).unwrap();
    card::build(&lab.home, id, &log).unwrap_or_else(|e| panic!("{}", e.msg))
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
    let want: Vec<Listed> = (files.iter())
        .map(|name| Listed {
            path: name.to_string(),
            sha256: Some(sha256_hex(&std::fs::read(dir.join(name)).unwrap())),
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

#[test]
fn absent_outside_file_listed_as_missing() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let dir = lab.home.study_dir(ID);
    std::fs::write(dir.join("statement.md"), "# statement\n").unwrap();
    let files = card_of(&lab).outside.files;
    let hash = sha256_hex(b"# statement\n");
    let want: Vec<Listed> = ["statement.md", "gaps.md", "decisions.md", "deviations.toml"]
        .iter()
        .map(|name| Listed {
            path: name.to_string(),
            sha256: (*name == "statement.md").then(|| hash.clone()),
        })
        .collect();
    assert_eq!(files, want);
}

#[test]
fn agent_not_applicable_below_level_2() {
    let lab = lab();
    fixture(
        &lab,
        MEAN,
        json!({ "isolation": 1, "launcher": "bwrap 0.9" }),
    );
    std::fs::write(lab.home.study_dir(ID).join("events.jsonl"), "{}\n").unwrap();
    let agent = card_of(&lab).setup.stack.agent_sha256;
    assert_eq!(agent, Field::NotApplicable);
    assert_eq!(agent.shown(|v| v.join(", ")), NOT_APPLICABLE);
    assert_eq!(NOT_APPLICABLE, "not applicable");
}

#[test]
fn card_reads_one_analysis() {
    let lab = lab();
    let mut all = bodies(&lab, MEAN, rows_on_level_2());
    // An earlier verdict and estimate, before the analysis record the card counts from.
    let early = json!({ "stratum": "code", "side": "below", "margin": MARGIN, "clusters": 4, "value": 999.0 });
    all.insert(
        1,
        (
            "verdict",
            json!({
                "hypothesis": "H1", "verdict": "pass", "basis": "test", "level": 0.01,
                "e_value": 999.0, "bounds": [early], "incomplete_units": 0, "reason": null,
            }),
        ),
    );
    let mut stale = estimate(&[("arm-a", 1.0, json!({}))], "code", 4.0, Some(0.1));
    stale["mean"] = json!(0.5);
    all.insert(1, ("estimate", stale));
    plant(&lab.home, ID, &all);
    let card = card_of(&lab);
    assert_eq!(card.hypotheses[0].verdict, "fail");
    assert_eq!(card.verdicts[0].display, "not shown");
    assert_eq!(card.claims[0].level, Some(0.005));
    let code = card.results.iter().find(|r| r.stratum == "code").unwrap();
    assert_eq!((code.estimate, code.e_value), (ESTIMATE, Some(3.1)));
    let placed = &card.sensitivity[0].placed;
    assert!(placed.iter().all(|p| p.estimate == ESTIMATE), "{placed:?}");
}

#[test]
fn runs_itemize_invalid_attempts() {
    let lab = lab();
    let mut all = bodies(&lab, MEAN, rows_on_level_2());
    all.push((
        "row",
        json!({
            "stage": "confirmation", "condition": "arm-a", "unit": "src/code-0.txt#9",
            "repeat": 0, "exit": "void:infra", "attempts": 1, "cpu_seconds": 0.5,
            "wall_seconds": 1.0, "reason": "the machine did not boot",
        }),
    ));
    all.push((
        "spend",
        json!({ "cpu_seconds": 1.5, "wall_seconds": 2.0, "reason": "row timed out" }),
    ));
    plant(&lab.home, ID, &all);
    let runs = card_of(&lab).runs;
    let ok: Vec<(String, u64)> = runs.ok.iter().map(|g| (g.stage.clone(), g.rows)).collect();
    assert_eq!(ok, vec![("confirmation".to_string(), 80)]);
    let want = vec![
        Attempt {
            row: None,
            kind: "row".into(),
            exit: Some("void:infra".into()),
            reason: Some("the machine did not boot".into()),
            cpu_seconds: Some(0.5),
        },
        Attempt {
            row: None,
            kind: "spend".into(),
            exit: None,
            reason: Some("row timed out".into()),
            cpu_seconds: Some(1.5),
        },
    ];
    assert_eq!(runs.attempts, want);
    assert_eq!(UNRECORDED, "Executions recorded nowhere cannot be listed.");
}

/// Telemetry whose tick counts rise by `25 x i` per field `i` at 100 ticks a second, with
/// `cgroup` as given.
fn telemetry(cgroup: Value) -> Value {
    let start: Vec<u64> = vec![1000; 10];
    let end: Vec<u64> = (0..10).map(|i| 1000 + 25 * i).collect();
    json!({
        "host_cpu": { "model": "Test CPU", "flags_sha256": "00" },
        "cpu_ticks": { "user_hz": 100, "start": start, "end": end },
        "cgroup": cgroup,
    })
}

#[test]
fn cost_totals_row_telemetry() {
    let unavailable = json!({ "unavailable": "level 2 rows run in a machine" });
    let lab = lab();
    fixture(&lab, MEAN, json!({ "telemetry": telemetry(unavailable) }));
    let t = card_of(&lab).cost.telemetry;
    assert_eq!((t.rows, t.recorded), (80, 80));
    assert_eq!(t.host_cpu.value, vec!["Test CPU".to_string()]);
    assert_eq!((t.cpu_ticks.read, t.cpu_ticks.unavailable), (80, 0));
    // steal is field 7: 175 ticks a row at 100 a second, over 80 rows.
    assert_eq!(t.cpu_ticks.value["steal"], 1.75 * 80.0);
    assert_eq!(t.cpu_ticks.value["user"], 0.0);
    assert_eq!(t.cpu_ticks.value.len(), 10);
    assert_eq!((t.cgroup.read, t.cgroup.unavailable), (0, 80));
    assert!(t.cgroup.value.is_empty(), "unavailable is not zero");
    assert_eq!(t.memory_peak_max, None);
    let read = json!({
        "memory_peak": 4096, "memory_events": { "max": 2, "oom_kill": 0 },
        "cpu_stat": { "usage_usec": 500, "user_usec": 300, "system_usec": 200 },
    });
    let lab = self::lab();
    fixture(&lab, MEAN, json!({ "telemetry": telemetry(read) }));
    let t = card_of(&lab).cost.telemetry;
    assert_eq!((t.cgroup.read, t.cgroup.unavailable), (80, 0));
    assert_eq!(t.cgroup.value["cpu_stat.usage_usec"], 500.0 * 80.0);
    assert_eq!(t.cgroup.value["memory_events.max"], 2.0 * 80.0);
    assert_eq!(t.memory_peak_max, Some(4096));
}

/// A payload whose `arm-a` value is 6 against the reference's 8.
const MINUS_25: &str = "#!/bin/sh\ncase \"$2\" in exact) v=8;; *) v=6;; esac\nprintf '{\"ppl\": %s}' \"$v\" > \"$IGNORARIUM_OUT/values.json\"\n";

#[test]
fn seeds_derive_from_the_plan_hash() {
    use ignorarium::backend::level0::Level0;
    use ignorarium::runner::{RunStatus, prepare_with, run};
    let lab = lab();
    common::pilot(&lab.home, vec![common::arm_a()]);
    let edit = |t: String| {
        common::confirmatory(t)
            .replace("units = 3", "units = 1")
            .replace("{ quantile = 0.5 }", "{ mean = [-30.0, 30.0] }")
            .replace("power_at = -20.0", "power_at = -25.0")
    };
    common::register_study(&lab, "mean", edit, &[], MINUS_25);
    let work = common::work(&lab, "mean");
    let plan = prepare_with(&lab.home, "mean", &work, vec![Box::new(Level0)]).unwrap();
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let card = card_of_study(&lab, "mean");
    let rows = common::rows_of(&lab, "mean").len() as u64;
    assert!(rows > 0);
    let seeds = &card.setup.seeds;
    assert_eq!(
        seeds.root,
        Field::Recorded {
            value: card.header.spec_hash.clone()
        }
    );
    assert_eq!(seeds.rule, ROW_SEED);
    assert_eq!((seeds.rows, seeds.derived), (rows, rows));
    let t = &card.cost.telemetry;
    assert_eq!((t.rows, t.recorded), (rows, rows));
}

#[test]
fn recipe_lists_missing_binary_hash() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let dir = lab.home.study_dir(ID);
    std::fs::write(dir.join("statement.md"), "# statement\n").unwrap();
    std::fs::write(dir.join("events.jsonl"), "{}\n").unwrap();
    std::fs::create_dir_all(dir.join("out")).unwrap();
    std::fs::write(dir.join("out/b.txt"), "b").unwrap();
    std::fs::write(dir.join("out/a.txt"), "a").unwrap();
    std::fs::write(dir.join("readings.md"), "## TL;DR\nblank\n").unwrap();
    let before = card_of(&lab).recipe;
    assert_eq!(before.command, "ignorarium run s1");
    assert_eq!(before.box_binary_sha256, Field::Missing { link: None });
    assert_eq!(before.bundle_doi, Field::Missing { link: None });
    assert_eq!(
        before.box_binary_sha256.shown(|v| v.clone()),
        NOT_IN_RECORDS
    );
    let version = card_of(&lab).surprises.box_versions[0].clone();
    assert_eq!(before.box_version, recorded(version));
    let files = &before.manifest.files;
    let paths: Vec<&str> = files.iter().map(|o| o.path.as_str()).collect();
    assert_eq!(
        paths,
        ["out/a.txt", "out/b.txt", "spec.toml", "statement.md"]
    );
    assert_eq!(files[0].sha256, sha256_hex(b"a"));
    assert_eq!(
        before.manifest.skipped,
        ["readings.md", "records.jsonl", "events.jsonl"]
    );
    std::fs::write(dir.join("readings.md"), "## TL;DR\nfilled\n").unwrap();
    std::fs::write(dir.join("events.jsonl"), "{}\n{}\n").unwrap();
    assert_eq!(card_of(&lab).recipe, before);
    std::fs::write(dir.join("out/a.txt"), "changed").unwrap();
    assert_ne!(card_of(&lab).recipe, before);
}

const CFF: &str = "cff-version: 1.2.0\nmessage: Cite it.\n# a comment\ntitle: \"Does X help?\"\n\
version: 0.3.0\nauthors:\n  - family-names: Lovelace\n    given-names: Ada\n    orcid: https://orcid.org/0000\n  - family-names: Babbage\n    given-names: Charles\nlicense: MIT\n";

#[test]
fn citation_from_cff() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    assert_eq!(card_of(&lab).citation.bibtex, Field::Missing { link: None });
    std::fs::write(lab.home.study_dir(ID).join("CITATION.cff"), CFF).unwrap();
    let c = card_of(&lab).citation;
    let bib = "@software{s1,\n  title = {Does X help?},\n  author = {Lovelace, Ada and Babbage, Charles},\n  version = {0.3.0}\n}\n";
    assert_eq!(c.bibtex, recorded(bib.to_string()));
    let cff = "title: \"Does X help?\"\nversion: 0.3.0\nauthors:\n  - family-names: Lovelace\n    given-names: Ada\n  - family-names: Babbage\n    given-names: Charles\n";
    assert_eq!(c.cff, recorded(cff.to_string()));
}

#[test]
fn citation_refuses_what_it_cannot_read() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let path = lab.home.study_dir(ID).join("CITATION.cff");
    let key = keys::load(&lab.home).unwrap().unwrap();
    let log_path = lab.home.study_dir(ID).join("records.jsonl");
    for bad in [
        "title: >\n  folded\nauthors:\n  - family-names: A\n    given-names: B\n",
        "title: T\nauthors:\n  - name: A Lab\n",
        "title: T\n",
        "authors:\n  - family-names: A\n    given-names: B\n",
        "title: T {x}\nauthors:\n  - family-names: A\n    given-names: B\n",
    ] {
        std::fs::write(&path, bad).unwrap();
        let log = records::read_log(&log_path, &key.verifying_key()).unwrap();
        let err = card::build(&lab.home, ID, &log).expect_err(bad);
        assert!(err.msg.contains("CITATION.cff"), "{}", err.msg);
    }
}
