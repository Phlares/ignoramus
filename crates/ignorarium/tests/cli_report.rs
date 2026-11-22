//! `report`: the verdicts of a finished study, through the real binary.
mod common;
use common::{
    Lab, bin, edited, finished_with, git, instrument, lab, plain, plant, records_of, source_spec,
    study_repo, text,
};
use serde_json::{Value, json};

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
fn report_refuses_while_the_home_holds_a_study_of_a_newer_format() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    plain(&lab, "old");
    common::reformat(&lab, "old", Some(2));
    let a = bin(&lab).args(["report", "s1"]).assert().failure();
    let out = a.get_output();
    let said = String::from_utf8_lossy(&out.stdout) + String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("old") && said.contains("format 2"), "{said}");
}

#[test]
fn a_finished_study_reports_each_verdict() {
    let lab = lab();
    let bodies = finished_study(&three(), &[]);
    plant(&lab.home, "s1", &bodies);
    let r = report_json(&lab, "s1");
    assert_eq!(r["study"], "s1");
    assert_eq!(r["finished_at"], ts_of(&lab));
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
        "h1: pass (test), e-value 312.4, level 0.005\n",
        "h2: fail (test), e-value 0, level 0.005: incomplete units in scope: 2\n",
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
fn small_numbers_print_exactly() {
    let lab = lab();
    let small = verdict(
        "h1",
        "pass",
        "test",
        json!(0.004),
        json!(0.000025),
        Value::Null,
    );
    plant(&lab.home, "s1", &finished_study(&[small], &[]));
    let a = bin(&lab).args(["report", "s1"]).assert().success();
    let t = text(&a);
    assert!(
        t.contains(
            "h1: pass (test), e-value 0.004, level 0.000025
"
        ),
        "{t}"
    );
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

/// `bodies` with a register record that names the plan `text`, which a test writes in place
/// of the planted one.
fn registered_as(mut bodies: Vec<(&'static str, Value)>, text: &str) -> Vec<(&'static str, Value)> {
    let hash = ignorarium::spec::parse(text.as_bytes()).unwrap().hash;
    let register = bodies.iter_mut().find(|(k, _)| *k == "register").unwrap();
    register.1["spec_hash"] = json!(hash);
    bodies
}

#[test]
fn a_blind_finished_study_shows_everything() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let dir = lab.home.study_dir("s2");
    std::fs::create_dir_all(&dir).unwrap();
    let blind = source_spec(&[]).replace("blind = false", "blind = true");
    assert!(blind.contains("blind = true"));
    std::fs::write(dir.join("spec.toml"), &blind).unwrap();
    plant(
        &lab.home,
        "s2",
        &registered_as(finished_study(&three(), &[]), &blind),
    );
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
    let bodies = finished_study(&[first], &[]);
    plant(&lab.home, "s1", &bodies);
    std::thread::sleep(std::time::Duration::from_millis(20));
    plant(&lab.home, "s1", &[("verdict", second)]);
    let r = report_json(&lab, "s1");
    let shown = r["hypotheses"].as_array().unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0]["verdict"], "pass");
    assert_eq!(shown[0]["e_value"], 312.4);
    let kept = records_of(&lab, "s1")
        .into_iter()
        .filter(|r| r.kind == "verdict")
        .map(|r| r.ts)
        .next()
        .unwrap();
    let last = records_of(&lab, "s1")
        .into_iter()
        .rfind(|r| r.kind == "verdict")
        .unwrap()
        .ts;
    assert!(kept < last, "{kept} {last}");
    assert_eq!(r["finished_at"], kept.as_str());
}

#[test]
fn an_analysis_that_promises_nothing_finishes_at_its_own_record() {
    let lab = lab();
    let mut bodies = finished_with(instrument(), None, vec![]);
    let analysis = bodies.iter_mut().find(|(k, _)| *k == "analysis").unwrap();
    analysis.1["estimates"] = json!(0);
    analysis.1["verdicts"] = json!(0);
    analysis.1["slot"] = Value::Null;
    plant(&lab.home, "s1", &bodies);
    let at = records_of(&lab, "s1")
        .into_iter()
        .find(|r| r.kind == "analysis")
        .unwrap()
        .ts;
    assert_eq!(report_json(&lab, "s1")["finished_at"], at.as_str());
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

fn child_estimate(child: &str, value: u64, mean: f64) -> Value {
    json!({
        "child": child, "parent": "arm-a", "operator": "window_tokens", "value": value,
        "outcome": "ppl-ratio", "clusters": 2, "units": 4, "mean": mean, "repeats": 1,
    })
}

/// A finished study whose child estimates come before its analysis record, as `finish` writes
/// them.
fn with_children(children: &[Value]) -> Vec<(&'static str, Value)> {
    let mut bodies = finished_study(&three(), &[]);
    let at = bodies.iter().position(|(k, _)| *k == "analysis").unwrap();
    for (i, c) in children.iter().enumerate() {
        bodies.insert(at + i, ("child-estimate", c.clone()));
    }
    bodies
}

#[test]
fn report_lists_children_with_their_estimates() {
    let lab = lab();
    let kids = [
        child_estimate("arm-a~window_tokens=64", 64, -12.5),
        child_estimate("arm-a~window_tokens=512", 512, 3.0),
    ];
    let bodies = with_children(&kids);
    plant(&lab.home, "s1", &bodies);
    let r = report_json(&lab, "s1");
    let listed: Vec<Value> = kids
        .iter()
        .map(|k| {
            let mut k = k.clone();
            k["stopped"] = json!(false);
            k
        })
        .collect();
    assert_eq!(r["children"], json!(listed));
    assert_eq!(r["records"], bodies.len());
    // The verdicts are as before.
    assert_eq!(r["hypotheses"].as_array().unwrap().len(), 3);
    let a = bin(&lab).args(["report", "s1"]).assert().success();
    let t = text(&a);
    for line in [
        "child arm-a~window_tokens=64: ppl-ratio mean -12.5 over 2 clusters, 4 units, 1 repeats\n",
        "child arm-a~window_tokens=512: ppl-ratio mean 3 over 2 clusters, 4 units, 1 repeats\n",
    ] {
        assert!(t.contains(line), "missing {line:?} in {t}");
    }
}

#[test]
fn a_child_named_in_a_spawn_stop_record_is_marked_stopped_and_partial() {
    let lab = lab();
    let kids = [
        child_estimate("arm-a~window_tokens=64", 64, -12.5),
        child_estimate("arm-a~window_tokens=512", 512, 3.0),
    ];
    let mut bodies = with_children(&kids);
    let at = bodies.iter().position(|(k, _)| *k == "analysis").unwrap();
    let stop = json!({ "children": ["arm-a~window_tokens=512"], "reason": "budget" });
    bodies.insert(at, ("spawn-stop", stop));
    plant(&lab.home, "s1", &bodies);
    let r = report_json(&lab, "s1");
    assert_eq!(r["children"][0]["stopped"], false);
    assert_eq!(r["children"][1]["stopped"], true);
    let a = bin(&lab).args(["report", "s1"]).assert().success();
    let t = text(&a);
    assert!(
        t.contains("child arm-a~window_tokens=512: ppl-ratio mean 3 over 2 clusters, 4 units, 1 repeats, stopped (partial: only the rows that ran)
"),
        "{t}"
    );
    assert!(
        t.contains(
            "1 repeats
"
        ) && t.contains(
            "window_tokens=64: ppl-ratio mean -12.5 over 2 clusters, 4 units, 1 repeats
"
        ),
        "{t}"
    );
}

#[test]
fn a_child_id_with_a_control_character_prints_escaped() {
    let lab = lab();
    let kids = [child_estimate("arm-a~x\u{1b}[2J", 64, 1.0)];
    plant(&lab.home, "s1", &with_children(&kids));
    let a = bin(&lab).args(["report", "s1"]).assert().success();
    let t = text(&a);
    assert!(!t.contains('\u{1b}'), "{t:?}");
    assert!(t.contains(r"child arm-a~x\u{1b}[2J: "), "{t:?}");
}

#[test]
fn a_report_without_children_lists_none() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    assert_eq!(report_json(&lab, "s1")["children"], json!([]));
    let a = bin(&lab).args(["report", "s1"]).assert().success();
    assert!(!text(&a).contains("child "), "{}", text(&a));
}

#[test]
fn a_blind_report_shows_children_by_the_blind_label_of_their_condition() {
    let lab = lab();
    let dir = lab.home.study_dir("s1");
    std::fs::create_dir_all(&dir).unwrap();
    let blind = source_spec(&["fiction".to_string()]).replace("blind = false", "blind = true");
    std::fs::write(dir.join("spec.toml"), &blind).unwrap();
    let kids = [child_estimate("arm-a~window_tokens=64", 64, -12.5)];
    plant(
        &lab.home,
        "s1",
        &registered_as(with_children(&kids), &blind),
    );
    let r = report_json(&lab, "s1");
    let child = r["children"][0]["child"].as_str().unwrap();
    let parent = r["children"][0]["parent"].as_str().unwrap();
    assert!(parent.starts_with("condition "), "{parent}");
    assert_eq!(child, format!("{parent}~window_tokens=64"));
    let a = bin(&lab).args(["report", "s1"]).assert().success();
    assert!(!text(&a).contains("arm-a"), "{}", text(&a));
    assert!(
        text(&a).contains(&format!("child {child}: ")),
        "{}",
        text(&a)
    );
}

#[test]
fn report_json_has_card_version() {
    let lab = lab();
    let bodies = finished_study(&three(), &[]);
    plant(&lab.home, "s1", &bodies);
    let a = bin(&lab)
        .args(["report", "s1", "--format", "json"])
        .assert()
        .success();
    let r: Value = serde_json::from_str(&text(&a)).unwrap();
    assert_eq!(r["card_version"], 2);
    assert_eq!(r["study"], "s1");
    assert_eq!(r["finished_at"], ts_of(&lab));
    assert_eq!(r["records"], bodies.len());
    assert_eq!(r["hypotheses"].as_array().unwrap().len(), 3);
    assert_eq!(r["hypotheses"][0]["e_value"], 312.4);
    assert_eq!(r["children"], json!([]));
    assert_eq!(r["family"], Value::Null);
    let opened = records_of(&lab, "s1").pop().unwrap();
    assert_eq!(r["opened"], json!({ "seq": opened.seq, "ts": opened.ts }));
}

#[test]
fn md_prints_the_markdown_card() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let a = bin(&lab)
        .args(["report", "s1", "--format", "md"])
        .assert()
        .success();
    let md = text(&a);
    assert!(md.starts_with("# Study card: s1\n"), "{md}");
    assert!(md.contains("## Re-run recipe"));
    let kinds: Vec<String> = records_of(&lab, "s1").into_iter().map(|r| r.kind).collect();
    assert!(kinds.contains(&"opened".to_string()), "{kinds:?}");
}

#[test]
fn tex_needs_an_out_folder_and_out_needs_tex() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    for args in [&["--format", "tex"][..], &["--format", "md", "--out", "x"]] {
        let a = bin(&lab).args(["report", "s1"]).args(args).assert().code(2);
        let err = String::from_utf8_lossy(&a.get_output().stderr).into_owned();
        assert!(err.contains("--out"), "{args:?}: {err}");
    }
    let kinds: Vec<String> = records_of(&lab, "s1").into_iter().map(|r| r.kind).collect();
    assert!(!kinds.contains(&"opened".to_string()), "{kinds:?}");
}

#[test]
fn tex_writes_its_files_to_the_out_folder() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let out = lab.home.study_dir("s1").join("tex-out");
    bin(&lab)
        .args(["report", "s1", "--format", "tex", "--out"])
        .arg(&out)
        .assert()
        .success();
    for name in ["results.tex", "deviations.tex", "sensitivity.tex"] {
        assert!(out.join(name).is_file(), "{name}");
    }
}

#[test]
fn tex_refuses_colliding_value_names_before_the_opened_record() {
    let lab = lab();
    let named = |h: &str| {
        let mut e = common::arm_a();
        e["hypothesis"] = json!(h);
        e
    };
    let bodies = finished_with(instrument(), None, vec![named("h 1"), named("h-1")]);
    plant(&lab.home, "s1", &bodies);
    let out = lab.root.join("tex-out");
    let a = bin(&lab)
        .args(["report", "s1", "--format", "tex", "--out"])
        .arg(&out)
        .assert()
        .code(2);
    let err = String::from_utf8_lossy(&a.get_output().stderr).into_owned();
    assert!(err.contains("h 1") && err.contains("h-1"), "{err}");
    no_opened(&lab);
}

/// A payload whose `arm-a` value is 6 against the reference's 8: every unit pairs to -25 %.
const MINUS_25: &str = "#!/bin/sh\ncase \"$2\" in exact) v=8;; *) v=6;; esac\nprintf '{\"ppl\": %s}' \"$v\" > \"$IGNORARIUM_OUT/values.json\"\n";

#[test]
fn card_marks_interval_post_hoc() {
    use common::run;
    use evidence::claim::{Grid, Statistic, tightest};
    use ignorarium::backend::level0::Level0;
    use ignorarium::runner::{RunStatus, prepare_with};
    let lab = lab();
    // `common::confirmatory`, one unit per cluster, on a mean in [-30, 30] sized at -25.
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
    let r = report_json(&lab, "mean");
    let h = &r["hypotheses"][0];
    assert_eq!(h["verdict"], "pass");
    assert_eq!(
        h.get("intervals"),
        None,
        "intervals live on results rows only"
    );
    let level = h["level"].as_f64().unwrap();
    // 15 clusters at -25: the same e-process at each margin from -25 up to 30.
    let grid = Grid {
        from: -25.0,
        to: 30.0,
        step: 1e-4,
    };
    let samples: Vec<&[f64]> = vec![&[-25.0]; 15];
    let mean = Statistic::Mean {
        lo: -30.0,
        hi: 30.0,
    };
    let found = tightest(&mean, evidence::Tail::Lower, &samples, 1.0 / level, grid).unwrap();
    assert!(found.margin.unwrap() < 0.0);
    let want = json!([{
        "stratum": "", "side": "below", "margin": 0.0, "estimate": -25.0, "bound": found.margin,
        "method": "inverted registered e-process",
        "grid": { "from": -25.0, "to": 30.0, "step": 1e-4 },
        "contiguous": true, "registered": false,
    }]);
    let pooled = (r["results"].as_array().unwrap().iter())
        .find(|row| row["hypothesis"] == h["hypothesis"] && row["stratum"] == "")
        .unwrap();
    assert_eq!(pooled["intervals"], want);
}

fn card_out(lab: &Lab, out: &std::path::Path) -> assert_cmd::assert::Assert {
    bin(lab)
        .args(["report", "s1", "--card", "--out"])
        .arg(out)
        .assert()
}

#[test]
fn card_out_writes_the_frozen_card_and_keeps_readings() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let out = lab.root.join("card");
    card_out(&lab, &out).success();
    for name in [
        "card.json",
        "card.md",
        "manifest.sha256",
        "data.sha256",
        "observations.csv",
        "readings.md",
    ] {
        assert!(out.join(name).is_file(), "{name}");
    }
    assert!(out.join("tex/results.tex").is_file());
    let bytes = std::fs::read(out.join("card.json")).unwrap();
    let card: Value = serde_json::from_slice(&bytes).unwrap();
    let sha = ignorarium::canon::sha256_hex;
    let manifest = std::fs::read(out.join("manifest.sha256")).unwrap();
    assert_eq!(sha(&manifest), card["recipe"]["manifest"]["list_sha256"]);
    let readings = std::fs::read_to_string(out.join("readings.md")).unwrap();
    assert!(readings.starts_with(&format!("<!-- card sha256: {} -->\n", sha(&bytes))));
    // A filled readings.md survives a second run; the card changes only by its opened record.
    std::fs::write(out.join("readings.md"), "mine\n").unwrap();
    card_out(&lab, &out).success();
    assert_eq!(
        std::fs::read_to_string(out.join("readings.md")).unwrap(),
        "mine\n"
    );
    let again = std::fs::read(out.join("card.json")).unwrap();
    assert_ne!(again, bytes);
    let frozen = |bytes: &[u8]| {
        let mut card: Value = serde_json::from_slice(bytes).unwrap();
        card["opened"] = Value::Null;
        card["records"] = Value::Null;
        serde_json::to_vec_pretty(&card).unwrap()
    };
    assert_eq!(
        String::from_utf8(frozen(&again)).unwrap(),
        String::from_utf8(frozen(&bytes)).unwrap(),
        "only opened and records may change"
    );
}

#[test]
fn data_sidecar_hash_matches_card() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let out = lab.root.join("card");
    card_out(&lab, &out).success();
    let card: Value =
        serde_json::from_slice(&std::fs::read(out.join("card.json")).unwrap()).unwrap();
    let sidecar =
        |name: &str| ignorarium::canon::sha256_hex(&std::fs::read(out.join(name)).unwrap());
    assert_eq!(sidecar("data.sha256"), card["setup"]["data"]["list_sha256"]);
    assert_eq!(sidecar("observations.csv"), card["observations"]["sha256"]);
}

#[test]
fn card_out_refuses_foreign_files_before_the_opened_record() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let out = lab.root.join("card");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("stale.txt"), "x").unwrap();
    let a = card_out(&lab, &out).code(2);
    let err = String::from_utf8_lossy(&a.get_output().stderr).into_owned();
    assert!(err.contains("stale.txt"), "{err}");
    let kinds: Vec<String> = records_of(&lab, "s1").into_iter().map(|r| r.kind).collect();
    assert!(!kinds.contains(&"opened".to_string()), "{kinds:?}");
}

#[test]
fn card_conflicts_with_format_and_needs_out() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    for args in [&["--card", "--format", "md", "--out", "x"][..], &["--card"]] {
        bin(&lab).args(["report", "s1"]).args(args).assert().code(2);
    }
}

fn stderr_of(a: &assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&a.get_output().stderr).into_owned()
}

fn no_opened(lab: &Lab) {
    let kinds: Vec<String> = records_of(lab, "s1").into_iter().map(|r| r.kind).collect();
    assert!(!kinds.contains(&"opened".to_string()), "{kinds:?}");
}

#[test]
fn card_out_refuses_a_foreign_file_in_a_nested_folder() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let out = lab.root.join("card");
    std::fs::create_dir_all(out.join("tex/sub")).unwrap();
    std::fs::write(out.join("tex/sub/x.txt"), "x").unwrap();
    let a = card_out(&lab, &out).code(2);
    assert!(stderr_of(&a).contains("sub"), "{}", stderr_of(&a));
    no_opened(&lab);
}

#[cfg(unix)]
#[test]
fn card_out_refuses_symlinks() {
    use std::os::unix::fs::symlink;
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let elsewhere = lab.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("f"), "x").unwrap();
    for (name, target) in [
        ("card.json", elsewhere.join("f")),
        ("tex", elsewhere.clone()),
    ] {
        let out = lab.root.join(format!("card-{name}"));
        std::fs::create_dir_all(&out).unwrap();
        symlink(&target, out.join(name)).unwrap();
        let a = card_out(&lab, &out).code(2);
        assert!(stderr_of(&a).contains("symlink"), "{}", stderr_of(&a));
    }
    no_opened(&lab);
    assert_eq!(std::fs::read(elsewhere.join("f")).unwrap(), b"x");
}

#[test]
fn readings_are_flagged_not_blocked_or_rewritten() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let out = lab.root.join("card");
    let a = card_out(&lab, &out).success();
    assert_eq!(stderr_of(&a), "", "a fresh template draws no flag");
    // Second run: the card changes (new opened record), so the header is stale. One slot has a digit.
    let path = out.join("readings.md");
    let old = std::fs::read_to_string(&path).unwrap();
    let mine = old.replacen("## TL;DR\n", "## TL;DR\n\nIt moved 5 points.\n", 1);
    std::fs::write(&path, &mine).unwrap();
    let a = card_out(&lab, &out).success();
    let err = stderr_of(&a);
    let new = ignorarium::canon::sha256_hex(&std::fs::read(out.join("card.json")).unwrap());
    let stale = old.lines().next().unwrap();
    let stale = stale
        .trim_start_matches("<!-- card sha256: ")
        .trim_end_matches(" -->");
    assert!(err.contains(stale) && err.contains(&new), "{err}");
    assert!(err.contains("It moved 5 points."), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), mine);
}

#[test]
fn flags_escape_the_authors_text_and_cut_it() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let out = lab.root.join("card");
    card_out(&lab, &out).success();
    let long = "x".repeat(200);
    let text = format!("## TL;DR\n\n\u{1b}[31m5 red {long}\n");
    std::fs::write(out.join("readings.md"), text).unwrap();
    let err = stderr_of(&card_out(&lab, &out).success());
    assert!(!err.contains('\u{1b}'), "{err:?}");
    assert!(err.contains("\\u{1b}[31m5 red"), "{err:?}");
    assert!(!err.contains(&"x".repeat(81)), "{err:?}");
}

#[cfg(unix)]
#[test]
fn an_unreadable_readings_file_is_flagged() {
    use std::os::unix::fs::PermissionsExt;
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let out = lab.root.join("card");
    card_out(&lab, &out).success();
    let path = out.join("readings.md");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable = std::fs::read(&path).is_ok();
    let err = stderr_of(&card_out(&lab, &out).success());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    // A root user reads the file anyway; the flag then has nothing to say.
    assert!(readable || err.contains("cannot read readings.md"), "{err}");
}

#[test]
fn study_folder_applies_to_every_format_and_to_the_card() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let (_repo, folder) = study_repo("sub", &[("gaps.md", "g\n")]);
    let commit = git(&folder, &["rev-parse", "HEAD"]);
    let with = |args: &[&str]| {
        let mut c = bin(&lab);
        c.args(["report", "s1"]).args(args).arg("--study-folder");
        c.arg(&folder).assert().success()
    };
    let json: Value = serde_json::from_str(&text(&with(&["--json"]))).unwrap();
    assert_eq!(
        json["outside_source"],
        json!({ "path": "sub/", "commit": commit })
    );
    assert!(text(&with(&["--format", "md"])).contains(&commit));
    with(&[]);
    let out = lab.root.join("card");
    let a = with(&["--card", "--out", out.to_str().unwrap()]);
    drop(a);
    let card: Value =
        serde_json::from_slice(&std::fs::read(out.join("card.json")).unwrap()).unwrap();
    assert_eq!(card["outside_source"]["commit"], commit.as_str());
}

#[test]
fn a_study_folder_refusal_comes_before_the_opened_record() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let (_repo, folder) = study_repo("", &[("gaps.md", "g\n")]);
    std::fs::write(folder.join("gaps.md"), "changed\n").unwrap();
    let a = bin(&lab)
        .args(["report", "s1", "--study-folder"])
        .arg(&folder)
        .assert()
        .failure();
    assert!(stderr_of(&a).contains("gaps.md"), "{}", stderr_of(&a));
    no_opened(&lab);
    let a = bin(&lab)
        .args(["report", "s1", "--study-folder"])
        .arg(lab.root.join("none"))
        .assert()
        .failure();
    assert!(stderr_of(&a).contains("not a folder"), "{}", stderr_of(&a));
    no_opened(&lab);
}

#[test]
fn a_missing_git_binary_is_a_refusal_that_says_so() {
    let lab = lab();
    plant(&lab.home, "s1", &finished_study(&three(), &[]));
    let (_repo, folder) = study_repo("", &[("gaps.md", "g\n")]);
    let empty = lab.root.join("no-bin");
    std::fs::create_dir_all(&empty).unwrap();
    let a = bin(&lab)
        .env("PATH", &empty)
        .args(["report", "s1", "--study-folder"])
        .arg(&folder)
        .assert()
        .failure();
    assert!(stderr_of(&a).contains("git not found"), "{}", stderr_of(&a));
    no_opened(&lab);
}
