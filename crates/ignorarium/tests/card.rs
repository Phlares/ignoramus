//! The card's sections, built from a finished confirmatory study with two strata.
mod common;
use common::{Lab, commit_all, estimate, finished_with, git, instrument, lab, plant, study_repo};
use ignorarium::canon::sha256_hex;
use ignorarium::card::sections::{
    Attempt, Field, Listed, NOT_APPLICABLE, NOT_IN_RECORDS, Outside, ROW_SEED, Source, UNRECORDED,
};
use ignorarium::card::{self, Card};
use ignorarium::derived::GatePrediction;
use ignorarium::rules::{Curve, CurvePoint};
use ignorarium::{keys, records, spec};
use serde_json::{Value, json};
use std::path::Path;

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
    build_in(lab, id, None).unwrap_or_else(|e| panic!("{}", e.msg))
}

fn build_in(lab: &Lab, id: &str, folder: Option<&Path>) -> Result<Card, ignorarium::cli::CliError> {
    let key = keys::load(&lab.home).unwrap().unwrap();
    let path = lab.home.study_dir(id).join("records.jsonl");
    let log = records::read_log(&path, &key.verifying_key()).unwrap();
    card::build(&lab.home, id, &log, folder)
}

/// The card of `ID` with its outside files read from `folder`.
fn card_in(lab: &Lab, folder: &Path) -> Card {
    build_in(lab, ID, Some(folder)).unwrap_or_else(|e| panic!("{}", e.msg))
}

/// The refusal `build` gives for `folder`.
fn refusal(lab: &Lab, folder: &Path) -> String {
    build_in(lab, ID, Some(folder)).expect_err("a refusal").msg
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

/// Issue #73: a row value the reader cannot parse is counted, not dropped without a trace.
#[test]
fn an_unreadable_row_value_is_counted() {
    let lab = lab();
    let mut bodies = bodies(&lab, MEAN, rows_on_level_2());
    let first = bodies.iter_mut().find(|(k, _)| *k == "row").unwrap();
    first.1["isolation"] = json!("2");
    plant(&lab.home, ID, &bodies);
    let card = card_of(&lab);
    let stack = &card.setup.stack;
    assert_eq!(stack.isolation, recorded(vec![2]));
    assert_eq!(
        stack.unreadable,
        std::collections::BTreeMap::from([("isolation".to_string(), 1)])
    );
    let md = card::render::markdown(&card);
    assert!(
        md.contains("Unreadable row values, rows per key: isolation 1"),
        "{md}"
    );
    let lab = self::lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let card = card_of(&lab);
    assert!(card.setup.stack.unreadable.is_empty());
    assert!(!card::render::markdown(&card).contains("Unreadable row values"));
}

/// Issue #80: the claim is held per stratum, so the pooled row carries no claim.
#[test]
fn a_row_outside_the_claim_is_descriptive() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let card = card_of(&lab);
    let marks: Vec<(&str, bool)> = (card.results.iter())
        .map(|r| (r.stratum.as_str(), r.descriptive))
        .collect();
    assert_eq!(marks, [("code", false), ("article", false), ("", true)]);
    let md = card::render::markdown(&card);
    let pooled = md.lines().find(|l| l.starts_with("| H1 | all |")).unwrap();
    assert!(pooled.ends_with("| descriptive |"), "{pooled}");
    let code = md.lines().find(|l| l.starts_with("| H1 | code |")).unwrap();
    assert!(code.ends_with("| registered |"), "{code}");
}

/// Issue #80: `manifest.sha256` lists paths of the study's folder in the box home.
#[test]
fn the_manifest_names_the_folder_it_checks() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let card = card_of(&lab);
    let md = card::render::markdown(&card);
    let check = format!(
        "```\n(cd <box home>/studies/{ID} && sha256sum -c) < <card folder>/manifest.sha256\n```"
    );
    assert!(md.contains(&check), "{md}");
}

/// Issue #80: the card names the box that built it, whatever box ran the rows.
#[test]
fn the_card_names_its_builder() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let card = card_of(&lab);
    let exe = std::env::current_exe().unwrap();
    let want = ignorarium::canon::sha256_file(&exe).unwrap();
    assert_eq!(
        card.builder.version,
        recorded(env!("CARGO_PKG_VERSION").to_string())
    );
    assert_eq!(card.builder.binary_sha256, recorded(want.clone()));
    let md = card::render::markdown(&card);
    let line = format!(
        "Card builder: version {}, binary SHA-256 {want}",
        env!("CARGO_PKG_VERSION")
    );
    assert!(md.contains(&line), "{md}");
}

/// Issue #80: the claim's frame prints beside it.
#[test]
fn the_population_prints_with_the_claim() {
    let lab = lab();
    let mut bodies = bodies(&lab, MEAN, rows_on_level_2());
    let dir = lab.home.study_dir(ID);
    let text = std::fs::read_to_string(dir.join("spec.toml")).unwrap();
    let text = text.replace("population = \"\"", "population = \"every input file\"");
    let hash = spec::parse(text.as_bytes()).unwrap().hash;
    for (kind, body) in bodies.iter_mut() {
        if *kind == "register" {
            body["spec_hash"] = json!(hash);
        }
    }
    std::fs::write(dir.join("spec.toml"), text).unwrap();
    plant(&lab.home, ID, &bodies);
    let card = card_of(&lab);
    assert_eq!(
        card.header.population,
        recorded("every input file".to_string())
    );
    let md = card::render::markdown(&card);
    let claim = md.split("## Claim\n").nth(1).unwrap();
    assert!(
        claim.starts_with("\n- Population: every input file\n"),
        "{claim}"
    );
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

const FIXED: [&str; 4] = ["gaps.md", "decisions.md", "deviations.toml", "CITATION.cff"];

fn present(path: &str, text: &str) -> Listed {
    Listed {
        path: path.into(),
        sha256: Some(sha256_hex(text.as_bytes())),
        note: None,
    }
}

fn absent(path: &str, note: Option<&str>) -> Listed {
    Listed {
        path: path.into(),
        sha256: None,
        note: note.map(String::from),
    }
}

#[test]
fn outside_files_hashed_not_parsed() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let said = "e-value 987654 shown";
    let text = |name: &str| match name {
        "deviations.toml" => String::new(),
        "CITATION.cff" => CFF.to_string(),
        _ => format!("# {name}\n{said}\n"),
    };
    let mut files: Vec<(&str, String)> = FIXED.iter().map(|n| (*n, text(n))).collect();
    files.push(("prereg/statement.md", text("statement.md")));
    let files: Vec<(&str, &str)> = files.iter().map(|(n, t)| (*n, t.as_str())).collect();
    let (_repo, folder) = study_repo("", &files);
    let dir = lab.home.study_dir(ID);
    std::fs::write(dir.join("readings.md"), format!("## TL;DR\n{said}\n")).unwrap();
    let card = card_in(&lab, &folder);
    let want: Vec<Listed> = files.iter().map(|(n, t)| present(n, t)).collect();
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
fn outside_files_from_git_folder() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let toml = "[[unregistered]]\nstep = \"s\"\nreason = \"r\"\ntiming = \"after-results\"\nsource = \"x\"\n";
    let (_repo, folder) = study_repo(
        "studies/e1",
        &[
            ("prereg/x-statement.md", "# s\n"),
            ("gaps.md", "g\n"),
            ("deviations.toml", toml),
        ],
    );
    let card = card_in(&lab, &folder);
    assert_eq!(
        card.outside.files,
        [
            present("gaps.md", "g\n"),
            absent("decisions.md", None),
            present("deviations.toml", toml),
            absent("CITATION.cff", None),
            present("prereg/x-statement.md", "# s\n"),
        ]
    );
    let want = Source {
        path: git(&folder, &["rev-parse", "--show-prefix"]),
        commit: git(&folder, &["rev-parse", "HEAD"]),
    };
    assert_eq!(want.path, "studies/e1/");
    assert_eq!(card.outside_source, Some(want));
    let rows: Vec<&str> = (card.deviations.iter())
        .map(|d| match d {
            card::DeviationRow::Unsigned { subject, .. } => subject.as_str(),
            _ => "other",
        })
        .collect();
    assert_eq!(rows, ["s"], "deviations.toml comes from the study folder");
}

/// Issue #80: the study's descriptive outputs are listed by path and hash, like `prereg/`.
#[test]
fn descriptive_outputs_are_listed() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let (_repo, folder) = study_repo(
        "",
        &[
            ("prereg/s.md", "s\n"),
            ("descriptive/b/rivals.json", "{}\n"),
            ("descriptive/a.json", "[]\n"),
        ],
    );
    let files = card_in(&lab, &folder).outside.files;
    let tail: Vec<Listed> = files[FIXED.len()..].to_vec();
    assert_eq!(
        tail,
        [
            present("prereg/s.md", "s\n"),
            present("descriptive/a.json", "[]\n"),
            present("descriptive/b/rivals.json", "{}\n"),
        ]
    );
    std::fs::write(folder.join("descriptive/a.json"), "changed\n").unwrap();
    assert!(refusal(&lab, &folder).contains("descriptive/a.json"));
}

#[test]
fn absent_outside_file_listed_as_missing() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let (_repo, folder) = study_repo("", &[("gaps.md", "g\n")]);
    let files = card_in(&lab, &folder).outside.files;
    let want: Vec<Listed> = (FIXED.iter())
        .map(|n| match *n {
            "gaps.md" => present(n, "g\n"),
            _ => absent(n, None),
        })
        .collect();
    assert_eq!(files, want);
}

#[test]
fn no_study_folder_marks_missing() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    // A file in the box folder is never read.
    std::fs::write(lab.home.study_dir(ID).join("gaps.md"), "g\n").unwrap();
    std::fs::write(lab.home.study_dir(ID).join("deviations.toml"), "[[x]]\n").unwrap();
    let card = card_of(&lab);
    let want: Vec<Listed> = FIXED
        .iter()
        .map(|n| absent(n, Some("no study folder given")))
        .collect();
    assert_eq!(card.outside.files, want);
    assert_eq!(card.outside_source, None);
    assert!(card.deviations.is_empty());
    assert_eq!(card.citation.bibtex, Field::Missing { link: None });
}

#[test]
fn uncommitted_outside_file_refused() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let (_repo, folder) = study_repo("", &[("gaps.md", "g\n"), ("prereg/s.md", "s\n")]);
    std::fs::write(folder.join("gaps.md"), "changed\n").unwrap();
    let said = refusal(&lab, &folder);
    assert!(
        said.contains("gaps.md") && said.contains("uncommitted"),
        "{said}"
    );
    commit_all(&folder);
    std::fs::write(folder.join("prereg/s.md"), "changed\n").unwrap();
    assert!(refusal(&lab, &folder).contains("prereg/s.md"));
}

#[test]
fn untracked_prereg_file_refused() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let (_repo, folder) = study_repo("", &[("prereg/s.md", "s\n")]);
    std::fs::write(folder.join("prereg/new.md"), "n\n").unwrap();
    let said = refusal(&lab, &folder);
    assert!(said.contains("prereg/new.md"), "{said}");
    // An ignored file is untracked too.
    std::fs::remove_file(folder.join("prereg/new.md")).unwrap();
    std::fs::write(folder.join(".gitignore"), "*.log\n").unwrap();
    commit_all(&folder);
    std::fs::write(folder.join("prereg/hid.log"), "n\n").unwrap();
    assert!(refusal(&lab, &folder).contains("prereg/hid.log"));
}

#[test]
fn not_a_git_folder_refused() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let plain = tempfile::TempDir::new().unwrap();
    std::fs::write(plain.path().join("gaps.md"), "g\n").unwrap();
    let said = refusal(&lab, plain.path());
    assert!(said.contains("git work tree"), "{said}");
    let file = plain.path().join("gaps.md");
    assert!(refusal(&lab, &file).contains("not a folder"));
    assert!(refusal(&lab, &plain.path().join("none")).contains("not a folder"));
    let (_repo, folder) = study_repo("", &[]);
    assert!(refusal(&lab, &folder.join(".git")).contains("git work tree"));
}

#[cfg(unix)]
#[test]
fn prereg_is_walked_sorted_and_links_are_not_followed() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let (_repo, folder) = study_repo(
        "",
        &[
            ("prereg/b.md", "b\n"),
            ("prereg/a/z.md", "z\n"),
            ("prereg/a.md", "a\n"),
        ],
    );
    std::os::unix::fs::symlink("/etc/hostname", folder.join("prereg/ln.md")).unwrap();
    std::os::unix::fs::symlink(".", folder.join("prereg/loop")).unwrap();
    std::os::unix::fs::symlink("gaps.md", folder.join("decisions.md")).unwrap();
    // A linked CITATION.cff would be refused if read; a linked deviations.toml would not parse.
    std::os::unix::fs::symlink("prereg/a.md", folder.join("CITATION.cff")).unwrap();
    std::os::unix::fs::symlink("prereg/a.md", folder.join("deviations.toml")).unwrap();
    commit_all(&folder);
    let files = card_in(&lab, &folder).outside.files;
    let link = |p: &str| absent(p, Some("link not followed"));
    assert_eq!(files[1], link("decisions.md"));
    assert_eq!(
        files[4..],
        [
            present("prereg/a.md", "a\n"),
            present("prereg/a/z.md", "z\n"),
            present("prereg/b.md", "b\n"),
            link("prereg/ln.md"),
            link("prereg/loop"),
        ]
    );
}

#[cfg(unix)]
#[test]
fn prereg_refuses_control_characters_and_a_fifo() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let (_repo, folder) = study_repo("", &[("prereg/ok.md", "s\n")]);
    std::fs::write(folder.join("prereg/a\nb"), "x").unwrap();
    let slash = char::from(92u8);
    assert!(refusal(&lab, &folder).contains(&format!("prereg/a{slash}nb")));
    std::fs::remove_file(folder.join("prereg/a\nb")).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(folder.join("prereg/pipe"))
        .status()
        .unwrap();
    assert!(made.success());
    assert!(refusal(&lab, &folder).contains("prereg/pipe"));
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
    let r = card_of(&lab).recipe;
    assert_eq!(r.command, "ignorarium run s1 --max-level 2");
    assert_eq!(r.isolation_levels, [2]);
    assert_eq!(r.requires.len(), 3);
    assert_eq!(r.box_binary_sha256, Field::Missing { link: None });
    assert_eq!(r.bundle_doi, Field::Missing { link: None });
    assert_eq!(r.box_binary_sha256.shown(|v| v.clone()), NOT_IN_RECORDS);
    let version = card_of(&lab).surprises.box_versions[0].clone();
    assert_eq!(r.box_version, recorded(version));
    let m = &r.manifest;
    assert_eq!(m.roots.len(), 7);
    assert_eq!(m.files, 1);
    assert_eq!(m.missing.len(), 6);
    assert!(!m.missing.contains(&"spec.toml".to_string()));
}

#[test]
fn recipe_box_version_is_the_read_analysis() {
    let lab = lab();
    let mut all = bodies(&lab, MEAN, rows_on_level_2());
    let mut later = all
        .iter()
        .find(|(k, _)| *k == "analysis")
        .unwrap()
        .1
        .clone();
    // Sorts before the read analysis's "0.1.0".
    later["box_version"] = json!("0.0.1");
    all.push(("analysis", later));
    plant(&lab.home, ID, &all);
    let card = card_of(&lab);
    assert_eq!(card.surprises.box_versions, ["0.0.1", "0.1.0"]);
    assert_eq!(card.recipe.box_version, recorded("0.1.0".to_string()));
}

#[test]
fn recipe_level_comes_from_the_rows() {
    let lab = lab();
    fixture(
        &lab,
        MEAN,
        json!({ "isolation": 1, "launcher": "bwrap 0.9" }),
    );
    let r = card_of(&lab).recipe;
    assert_eq!(r.command, "ignorarium run s1 --max-level 1");
    let lab = lab_with_levels();
    let r = card_of(&lab).recipe;
    assert_eq!(r.isolation_levels, [1, 2]);
    assert!(r.command.ends_with("--max-level 2"));
}

fn lab_with_levels() -> Lab {
    let lab = lab();
    let mut bodies = bodies(&lab, MEAN, rows_on_level_2());
    let first = bodies.iter_mut().find(|(k, _)| *k == "row").unwrap();
    first.1["isolation"] = json!(1);
    plant(&lab.home, ID, &bodies);
    lab
}

fn manifest_of(lab: &Lab) -> ignorarium::card::recipe::Manifest {
    card_of(lab).recipe.manifest
}

#[test]
fn manifest_ignores_what_a_run_rewrites() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let dir = lab.home.study_dir(ID);
    let before = manifest_in(&dir);
    std::fs::create_dir_all(dir.join("run")).unwrap();
    std::fs::write(dir.join("run/control"), "pause").unwrap();
    std::fs::write(dir.join("deviations.jsonl"), "{}\n").unwrap();
    std::fs::write(dir.join("readings.md"), "filled\n").unwrap();
    std::fs::write(dir.join("events.jsonl"), "{}\n").unwrap();
    assert_eq!(manifest_in(&dir), before);
    std::fs::create_dir_all(dir.join("payload")).unwrap();
    std::fs::write(dir.join("payload/run.sh"), "a").unwrap();
    let with = manifest_in(&dir);
    assert_ne!(with.list_sha256, before.list_sha256);
    assert_eq!(with.files, before.files + 1);
    assert!(!with.missing.contains(&"payload/".to_string()));
    std::fs::write(dir.join("payload/run.sh"), "b").unwrap();
    assert_ne!(manifest_in(&dir).list_sha256, with.list_sha256);
}

#[test]
fn manifest_lines_match_the_tree_hash() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let dir = lab.home.study_dir(ID);
    std::fs::create_dir_all(dir.join("in/sub")).unwrap();
    std::fs::write(dir.join("in/sub/b"), "b").unwrap();
    std::fs::write(dir.join("in/a"), "a").unwrap();
    let lines = card::recipe::manifest_lines(&dir).unwrap();
    let paths: Vec<&str> = lines
        .iter()
        .map(|l| l.split_once("  ").unwrap().1)
        .collect();
    assert_eq!(
        paths,
        ["in/a", "in/sub/b", "spec.toml"]
            .map(String::from)
            .map(|s| s.leak() as &str)
    );
    assert!(lines[0].starts_with(&sha256_hex(b"a")));
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    assert_eq!(manifest_of(&lab).list_sha256, sha256_hex(text.as_bytes()));
    assert_eq!(manifest_of(&lab).files, 3);
}

#[cfg(unix)]
#[test]
fn manifest_records_links_and_never_follows_them() {
    use std::os::unix::fs::symlink;
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let dir = lab.home.study_dir(ID);
    std::fs::create_dir_all(dir.join("in")).unwrap();
    symlink("/etc/hostname", dir.join("in/host")).unwrap();
    symlink(".", dir.join("in/loop")).unwrap();
    symlink("nowhere", dir.join("payload")).unwrap();
    let lines = card::recipe::manifest_lines(&dir).unwrap();
    let link = |t: &str, p: &str| format!("link:{}  {p}", sha256_hex(t.as_bytes()));
    assert!(lines.contains(&link("/etc/hostname", "in/host")));
    assert!(lines.contains(&link(".", "in/loop")));
    assert!(lines.contains(&link("nowhere", "payload")));
    assert_eq!(lines.len(), 4);
}

#[cfg(unix)]
#[test]
fn manifest_refuses_a_fifo() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let dir = lab.home.study_dir(ID);
    std::fs::create_dir_all(dir.join("in")).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(dir.join("in/pipe"))
        .status()
        .unwrap();
    assert!(made.success());
    let err = card::recipe::manifest_lines(&dir).unwrap_err();
    assert!(err.contains("in/pipe"), "{err}");
}

const CFF: &str = "cff-version: 1.2.0\nmessage: Cite it.\n# a comment\ntitle: \"Does X help?\"\n\
version: 0.3.0\nauthors:\n  - family-names: Lovelace\n    given-names: Ada\n    orcid: https://orcid.org/0000\n  - family-names: Babbage\n    given-names: Charles\nlicense: MIT\n";

#[test]
fn citation_from_cff() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let (_repo, folder) = study_repo("", &[]);
    let missing = Field::Missing { link: None };
    assert_eq!(card_in(&lab, &folder).citation.bibtex, missing);
    std::fs::write(folder.join("CITATION.cff"), CFF).unwrap();
    commit_all(&folder);
    let c = card_in(&lab, &folder).citation;
    let bib = "@software{s1,\n  title = {Does X help?},\n  author = {Lovelace, Ada and Babbage, Charles},\n  version = {0.3.0}\n}\n";
    assert_eq!(c.bibtex, recorded(bib.to_string()));
    let cff = "title: \"Does X help?\"\nversion: 0.3.0\nauthors:\n  - family-names: Lovelace\n    given-names: Ada\n  - family-names: Babbage\n    given-names: Charles\n";
    assert_eq!(c.cff, recorded(cff.to_string()));
}

#[test]
fn citation_refuses_what_it_cannot_read() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let (_repo, folder) = study_repo("", &[]);
    let path = folder.join("CITATION.cff");
    for bad in [
        "title: >\n  folded\nauthors:\n  - family-names: A\n    given-names: B\n",
        "title: T\nauthors:\n  - name: A Lab\n",
        "title: T\n",
        "authors:\n  - family-names: A\n    given-names: B\n",
        "title: T {x}\nauthors:\n  - family-names: A\n    given-names: B\n",
    ] {
        std::fs::write(&path, bad).unwrap();
        commit_all(&folder);
        let err = refusal(&lab, &folder);
        assert!(err.contains("CITATION.cff"), "{err}");
    }
}

fn manifest_in(dir: &std::path::Path) -> card::recipe::Manifest {
    let (lines, missing) = card::recipe::walk(dir).unwrap();
    card::recipe::recipe(&lines, missing, &[], ID, &[2], "0.1.0").manifest
}

#[cfg(unix)]
#[test]
fn manifest_refuses_control_characters() {
    use std::os::unix::fs::symlink;
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let dir = lab.home.study_dir(ID);
    std::fs::create_dir_all(dir.join("in")).unwrap();
    std::fs::write(dir.join("in/a\nb"), "x").unwrap();
    let err = card::recipe::manifest_lines(&dir).unwrap_err();
    let slash = char::from(92u8);
    assert!(err.contains(&format!("in/a{slash}nb")), "{err}");
    std::fs::remove_file(dir.join("in/a\nb")).unwrap();
    symlink("t\rx", dir.join("in/link")).unwrap();
    let err = card::recipe::manifest_lines(&dir).unwrap_err();
    assert!(err.contains(&format!("t{slash}rx")), "{err}");
}

fn sidecars_of(lab: &Lab) -> card::sidecar::Sidecars {
    let key = keys::load(&lab.home).unwrap().unwrap();
    let path = lab.home.study_dir(ID).join("records.jsonl");
    let log = records::read_log(&path, &key.verifying_key()).unwrap();
    card::sidecar::Sidecars::of(&lab.home, ID, &log).unwrap()
}

/// The signed input lines of the fixture: each present input, by path.
fn signed_lines() -> Vec<String> {
    let mut paths: Vec<String> = (strata().iter())
        .flat_map(|s| (0..4).map(move |i| format!("src/{s}-{i}.txt")))
        .collect();
    paths.sort();
    (paths.iter())
        .map(|p| format!("{}  {p}", sha256_hex(p.as_bytes())))
        .collect()
}

#[test]
fn card_data_is_count_and_list_hash() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let card = card_of(&lab);
    let text: String = signed_lines()
        .iter()
        .map(|l| {
            format!(
                "{l}
"
            )
        })
        .collect();
    let json = serde_json::to_value(&card.setup.data).unwrap();
    assert_eq!(
        json,
        json!({ "count": 8, "absent": 2, "list_sha256": sha256_hex(text.as_bytes()), "file": "data.sha256" })
    );
    assert_eq!(sidecars_of(&lab).data, text);
}

/// Writes each signed input into `in/`: its content is its path, which hashes to its signed
/// digest.
fn fill_in(lab: &Lab) {
    let dir = lab.home.study_dir(ID).join("in");
    for line in signed_lines() {
        let path = line.split_once("  ").unwrap().1.to_string();
        std::fs::create_dir_all(dir.join(&path).parent().unwrap()).unwrap();
        std::fs::write(dir.join(&path), &path).unwrap();
    }
    std::fs::write(dir.join("ignorarium:inputs.json"), "[]").unwrap();
}

fn in_folder(lab: &Lab) -> card::recipe::Agreement {
    card_of(lab).recipe.manifest.in_folder
}

fn agreement(missing: u64, extra: u64, differ: u64) -> card::recipe::Agreement {
    card::recipe::Agreement {
        agrees: missing + extra + differ == 0,
        missing,
        extra,
        differ,
    }
}

#[test]
fn in_agrees_with_signed_list() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    fill_in(&lab);
    assert_eq!(in_folder(&lab), agreement(0, 0, 0));
}

#[test]
fn in_differs_flagged() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    fill_in(&lab);
    let dir = lab.home.study_dir(ID).join("in");
    std::fs::write(dir.join("src/code-1.txt"), "other").unwrap();
    assert_eq!(in_folder(&lab), agreement(0, 0, 1));
    std::fs::write(dir.join("stray.txt"), "x").unwrap();
    assert_eq!(in_folder(&lab), agreement(0, 1, 1));
    std::fs::remove_file(dir.join("src/code-0.txt")).unwrap();
    assert_eq!(in_folder(&lab), agreement(1, 1, 1));
}

/// The observation of cluster `src/<stratum>-<k>.txt` in the fixture below.
fn observed_at(stratum: &str, k: usize) -> f64 {
    0.01 * (k + 1) as f64 + if stratum == "code" { 0.0 } else { 0.02 }
}

/// The statistic of `MEAN`: the plan's range as written, not ln 2.
#[allow(clippy::approx_constant)]
fn plan_stat() -> evidence::claim::Statistic {
    evidence::claim::Statistic::Mean {
        lo: -0.693147,
        hi: 0.693147,
    }
}

/// Gives each cluster of the fixture its own `observed_at` value.
fn varied(all: &mut [(&'static str, Value)]) {
    for (_, body) in all.iter_mut().filter(|(k, _)| *k == "row") {
        let unit = body["unit"].as_str().unwrap().to_string();
        let (path, _) = unit.split_once('#').unwrap();
        let name = path.trim_start_matches("src/").trim_end_matches(".txt");
        let (stratum, k) = name.split_once('-').unwrap();
        if body["condition"] == "arm-a" {
            body["values"]["ppl"] = json!(10.0 * observed_at(stratum, k.parse().unwrap()).exp());
        }
    }
}

#[test]
fn observations_csv_recomputes_e_value() {
    use evidence::Tail;
    use evidence::claim::{Bound, e_value, order};
    use evidence::rng::Rng;
    let stat = plan_stat();
    let bound = Bound {
        tail: Tail::Lower,
        margin: MARGIN,
    };
    let lab = lab();
    let mut all = bodies(&lab, MEAN, rows_on_level_2());
    let hash = spec::hash_bytes(&spec::parse(spec_text(MEAN).as_bytes()).unwrap().hash).unwrap();
    varied(&mut all);
    // The test reads each stratum's clusters in a seeded order; the registered e-value is the
    // least over the strata, from observations the test of the fixture's plan would see.
    let seeded = |stratum: &str| -> Vec<f64> {
        let path = format!("test/H1/{stratum}");
        let rng = &mut Rng::from_seed(ignorarium::seed::seed(&hash, &path));
        order(4, rng)
            .into_iter()
            .map(|k| observed_at(stratum, k))
            .collect()
    };
    let want = ["code", "article"]
        .map(|s| e_value(&stat, &bound, &seeded(s)).unwrap())
        .into_iter()
        .fold(f64::INFINITY, f64::min);
    all.iter_mut().find(|(k, _)| *k == "verdict").unwrap().1["e_value"] = json!(want);
    plant(&lab.home, ID, &all);
    let csv = sidecars_of(&lab).observations;
    let mut lines = csv.lines();
    assert_eq!(
        lines.next(),
        Some("hypothesis,stratum,side,position,cluster,units,observation")
    );
    let mut groups: std::collections::BTreeMap<String, Vec<(u64, f64)>> = Default::default();
    for line in lines {
        let f: Vec<&str> = line.split(',').collect();
        assert_eq!(f.len(), 7, "{line}");
        assert_eq!((f[0], f[2], f[5]), ("H1", "below", "5"), "{line}");
        groups
            .entry(f[1].into())
            .or_default()
            .push((f[3].parse().unwrap(), f[6].parse().unwrap()));
    }
    assert_eq!(groups.len(), 2);
    let e = groups
        .values()
        .map(|g| {
            let positions: Vec<u64> = g.iter().map(|(p, _)| *p).collect();
            assert_eq!(positions, [1, 2, 3, 4], "rows come in test order");
            let obs: Vec<f64> = g.iter().map(|(_, x)| *x).collect();
            e_value(&stat, &bound, &obs).unwrap()
        })
        .fold(f64::INFINITY, f64::min);
    assert!((e / want - 1.0).abs() < 1e-9, "{e} against {want}");
    assert_eq!(card_of(&lab).hypotheses[0].e_value, Some(want));
}

#[test]
fn observations_hash_is_on_the_card() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let card = card_of(&lab);
    let csv = sidecars_of(&lab).observations;
    assert_eq!(card.observations.sha256, sha256_hex(csv.as_bytes()));
    assert_eq!(card.observations.rows, 8);
    assert_eq!(card.observations.file, "observations.csv");
}

#[test]
fn card_md_has_no_long_lists() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let md = card::render::markdown(&card_of(&lab));
    assert!(!md.contains("src/code-0.txt"), "{md}");
    let digests = md
        .split(|c: char| !c.is_ascii_hexdigit())
        .filter(|w| w.len() == 64)
        .count();
    assert!(digests <= 11, "{digests} digests in the card");
}

#[test]
fn inputs_without_a_digest_are_counted_and_printed() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let card = card_of(&lab);
    assert_eq!(card.setup.data.absent, 2);
    assert!(card::render::markdown(&card).contains("2 inputs without a digest"));
}

/// The fixture with `H1`'s registered verdict forced to e-value 0 for `reason`.
fn forced_lab(reason: &str) -> Lab {
    let lab = lab();
    let mut all = bodies(&lab, MEAN, rows_on_level_2());
    let v = &mut all.iter_mut().find(|(k, _)| *k == "verdict").unwrap().1;
    (v["e_value"], v["reason"]) = (json!(0.0), json!(reason));
    plant(&lab.home, ID, &all);
    lab
}

#[test]
fn a_forced_verdict_is_marked_beside_the_observations() {
    let lab = forced_lab("stopped: out of budget");
    let card = card_of(&lab);
    let want = vec![card::sidecar::Forced {
        hypothesis: "H1".into(),
        reason: "stopped: out of budget".into(),
    }];
    assert_eq!(card.observations.forced, want);
    let md = card::render::markdown(&card);
    assert!(md.contains("raw observations"), "{md}");
    assert!(md.contains("stopped: out of budget"), "{md}");
}

#[test]
fn a_real_zero_with_a_tree_split_reason_is_not_marked_forced() {
    let lab = forced_lab("the study's level does not split over its claims: no share");
    let card = card_of(&lab);
    assert!(card.observations.forced.is_empty());
    assert!(!card::render::markdown(&card).contains("raw observations"));
}

#[test]
fn an_unforced_verdict_is_not_marked() {
    let lab = lab();
    fixture(&lab, MEAN, rows_on_level_2());
    let card = card_of(&lab);
    assert!(card.observations.forced.is_empty());
    assert!(!card::render::markdown(&card).contains("raw observations"));
}

#[test]
fn observations_csv_recomputes_interval() {
    use evidence::Tail;
    use evidence::claim::{Grid, tightest};
    let stat = plan_stat();
    let lab = lab();
    let mut all = bodies(&lab, MEAN, rows_on_level_2());
    varied(&mut all);
    // A level the four clusters of a stratum can reach, so each interval has a bound.
    all.iter_mut().find(|(k, _)| *k == "verdict").unwrap().1["level"] = json!(0.9);
    plant(&lab.home, ID, &all);
    let card = card_of(&lab);
    let csv = sidecars_of(&lab).observations;
    let threshold = 1.0 / card.hypotheses[0].level.unwrap();
    let mut checked = 0;
    for row in card
        .results
        .iter()
        .filter(|r| r.hypothesis == "H1" && !r.stratum.is_empty())
    {
        let mut seen: Vec<(u64, f64)> = csv
            .lines()
            .skip(1)
            .map(|l| l.split(',').collect::<Vec<_>>())
            .filter(|f| f[1] == row.stratum)
            .map(|f| (f[3].parse().unwrap(), f[6].parse().unwrap()))
            .collect();
        seen.sort_by_key(|(position, _)| *position);
        let cells: Vec<[f64; 1]> = seen.iter().map(|(_, x)| [*x]).collect();
        let samples: Vec<&[f64]> = cells.iter().map(|c| c.as_slice()).collect();
        for i in &row.intervals {
            assert_eq!(i.side, "below");
            let g = Grid {
                from: i.grid.from,
                to: i.grid.to,
                step: i.grid.step,
            };
            let found = tightest(&stat, Tail::Lower, &samples, threshold, g).unwrap();
            assert!(
                found.margin.is_some(),
                "the fixture's interval is not empty"
            );
            assert_eq!(found.margin.filter(|_| found.contiguous), i.bound);
            checked += 1;
        }
    }
    assert_eq!(checked, 2);
}
