use ignorarium::diag::{Diagnostic, Severity, render_human, render_json};
use ignorarium::rules::{self, Check, Rule};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::Path;

fn v0() -> Vec<Rule> {
    rules::load("../../rules/v0.toml".as_ref()).unwrap()
}

fn spec_at(path: &str) -> Value {
    ignorarium::spec::load(Path::new(path)).unwrap().value
}

fn template() -> Value {
    spec_at("../../templates/numeric-ablation.toml")
}

fn codes_paths(d: &[Diagnostic]) -> Vec<(&str, &str)> {
    d.iter()
        .map(|x| (x.code.as_str(), x.path.as_str()))
        .collect()
}

/// Rules from inline TOML, for tests that need their own rule set.
fn local_rules(text: &str) -> Vec<Rule> {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("r.toml");
    std::fs::write(&file, text).unwrap();
    rules::load(&file).unwrap()
}

const HEAD: &str = "code = \"X-TST-001\"\nseverity = \"error\"\nproblem = \"p\"\ninstruction = \"i\"\nwhy = \"w\"\nsources = [\"https://example.org\"]\n";

fn one_rule(extra: &str, target: &str) -> Vec<Rule> {
    local_rules(&format!(
        "[[rule]]\n{HEAD}{extra}\n[[rule.target]]\n{target}\n"
    ))
}

#[test]
fn each_bad_fixture_yields_its_code_and_path() {
    let rules = v0();
    for (code, path) in [
        ("E-REQ-001", "study.mode"),
        ("E-TYP-001", "design.repeats"),
        ("E-ENM-001", "study.mode"),
        ("E-RNG-001", "design.alpha"),
        ("E-FMT-001", "study.id"),
        ("E-NUM-001", "design.sesoi"),
        ("E-OUT-001", "outcomes[0].unit"),
        ("E-OUT-002", "outcomes[0].direction"),
        ("W-LIC-001", "inputs[0].licence.spdx"),
        ("E-KEY-001", "study.surprise"),
        ("E-FMT-002", "design.variance.source"),
        ("E-FMT-003", "inputs[0].sha256"),
    ] {
        let raw = spec_at(&format!("tests/fixtures/specs/bad/{code}.toml"));
        let d = rules::check_generic(&raw, &rules);
        assert!(
            d.iter().any(|x| x.code == code && x.path == path),
            "{code}: {d:?}"
        );
        // One fault, one diagnostic: no cascade.
        assert_eq!(
            d.iter().filter(|x| x.code != "W-LIC-001").count(),
            usize::from(code != "W-LIC-001"),
            "{code}: {d:?}"
        );
    }
}

#[test]
fn templates_have_no_generic_errors_and_no_unknown_keys() {
    let rules = v0();
    for t in ["numeric-ablation", "planted-spawn"] {
        let d = rules::check_generic(&spec_at(&format!("../../templates/{t}.toml")), &rules);
        assert_eq!(codes_paths(&d), [("W-LIC-001", "inputs[0].licence.spdx")]);
        assert_eq!(d[0].severity, Severity::Warning);
    }
}

#[test]
fn all_faults_reported_at_once_in_rule_file_order() {
    let d = rules::check_generic(
        &spec_at("tests/fixtures/specs/bad/three-faults.toml"),
        &v0(),
    );
    assert_eq!(
        codes_paths(&d),
        [
            ("E-REQ-001", "study.mode"),
            ("E-RNG-001", "design.alpha"),
            ("E-FMT-001", "study.id"),
            ("W-LIC-001", "inputs[0].licence.spdx"),
        ]
    );
}

#[test]
fn diagnostic_carries_rule_text_sources_and_doc() {
    let d = rules::check_generic(&spec_at("tests/fixtures/specs/bad/E-OUT-001.toml"), &v0());
    let x = d.iter().find(|x| x.code == "E-OUT-001").unwrap();
    assert_eq!(x.severity, Severity::Error);
    assert_eq!(x.problem, "An outcome has no unit.");
    assert_eq!(
        x.instruction,
        "Give every outcome a unit, for example ratio or seconds."
    );
    assert_eq!(x.doc, "docs/rules/E-OUT-001.md");
    assert_eq!(x.sources, ["https://doi.org/10.1136/bmj.e7586"]);
}

#[test]
fn json_render_round_trips() {
    let d = rules::check_generic(
        &spec_at("tests/fixtures/specs/bad/three-faults.toml"),
        &v0(),
    );
    let back: Value = serde_json::from_str(&render_json(&d)).unwrap();
    let got: Vec<(&str, &str)> = back
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x["code"].as_str().unwrap(), x["path"].as_str().unwrap()))
        .collect();
    assert_eq!(got, codes_paths(&d));
    assert_eq!(back[0]["severity"], "error");
    assert_eq!(back[3]["severity"], "warning");
    assert_eq!(back[0]["doc"], "docs/rules/E-REQ-001.md");
}

#[test]
fn human_render_matches_exactly() {
    let mk = |code: &str, sev, path: &str| Diagnostic {
        code: code.into(),
        severity: sev,
        path: path.into(),
        problem: "Problem.".into(),
        instruction: "Do this.".into(),
        doc: format!("docs/rules/{code}.md"),
        sources: vec![],
    };
    let out = render_human(&[
        mk("E-OUT-001", Severity::Error, "outcomes[1].unit"),
        mk("W-LIC-001", Severity::Warning, "inputs[0].licence.spdx"),
    ]);
    assert_eq!(
        out,
        "error[E-OUT-001] outcomes[1].unit: Problem.\n  fix: Do this.\n  see: docs/rules/E-OUT-001.md\n\n\
         warning[W-LIC-001] inputs[0].licence.spdx: Problem.\n  fix: Do this.\n  see: docs/rules/W-LIC-001.md\n"
    );
    assert_eq!(render_human(&[]), "");
}

#[test]
fn no_duplicate_path_and_kind_pairs() {
    let mut seen = HashSet::new();
    for r in v0() {
        assert!(
            seen.insert((r.path.clone(), r.check.name())),
            "{}: {}",
            r.code,
            r.path
        );
    }
}

#[test]
fn every_template_key_has_a_type_target() {
    fn paths(v: &Value, prefix: &str, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                for (k, c) in m {
                    let p = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    out.push(p.clone());
                    paths(c, &p, out);
                }
            }
            Value::Array(a) => {
                for e in a.iter().filter(|e| e.is_object()) {
                    paths(e, &format!("{prefix}[*]"), out);
                }
            }
            _ => {}
        }
    }
    let types: HashSet<String> = v0()
        .into_iter()
        .filter(|r| matches!(r.check, Check::Type(_)))
        .map(|r| r.path)
        .collect();
    for t in ["numeric-ablation", "planted-spawn"] {
        let mut all = vec![];
        paths(&spec_at(&format!("../../templates/{t}.toml")), "", &mut all);
        let missing: Vec<_> = all.iter().filter(|p| !types.contains(*p)).collect();
        assert!(missing.is_empty(), "{t}: {missing:?}");
    }
}

#[test]
fn fields_with_defaults_are_never_required() {
    let required: HashSet<String> = v0()
        .into_iter()
        .filter(|r| matches!(r.check, Check::Required))
        .map(|r| r.path)
        .collect();
    for p in [
        "design.alpha",
        "design.power",
        "design.repeats",
        "mutations.depth_cap",
        "scope.min_isolation",
        "access.blind",
        "outcomes[*].unit",
        "outcomes[*].direction",
    ] {
        assert!(!required.contains(p), "{p}");
    }
    assert!(required.contains("study.mode"));
}

fn with(path: &[&str], v: Value) -> Value {
    let mut s = template();
    let mut cur = &mut s;
    for k in &path[..path.len() - 1] {
        cur = &mut cur[*k];
    }
    cur[path[path.len() - 1]] = v;
    s
}

fn rng_codes(spec: &Value) -> Vec<String> {
    rules::check_generic(spec, &v0())
        .into_iter()
        .filter(|d| d.code == "E-RNG-001")
        .map(|d| d.path)
        .collect()
}

#[test]
fn range_bounds_hold_on_both_sides() {
    let cases: [(&[&str], Value, bool); 18] = [
        (&["design", "alpha"], json!(0), true),
        (&["design", "alpha"], json!(0.0000001), false),
        (&["design", "alpha"], json!(0.5), false),
        (&["design", "alpha"], json!(0.5000001), true),
        (&["design", "power"], json!(0.4999), true),
        (&["design", "power"], json!(0.5), false),
        (&["design", "power"], json!(0.999), false),
        (&["design", "power"], json!(0.9991), true),
        (&["design", "icc"], json!(-0.0001), true),
        (&["design", "icc"], json!(0), false),
        (&["design", "icc"], json!(0.9999), false),
        (&["design", "icc"], json!(1), true),
        (&["design", "repeats"], json!(0), true),
        (&["design", "repeats"], json!(1), false),
        (&["design", "repeats"], json!(1000000), false),
        (&["scope", "min_isolation"], json!(-1), true),
        (&["scope", "min_isolation"], json!(3), false),
        (&["scope", "min_isolation"], json!(4), true),
    ];
    for (path, v, bad) in cases {
        let got = rng_codes(&with(path, v.clone()));
        let want = if bad { vec![path.join(".")] } else { vec![] };
        assert_eq!(got, want, "{path:?} = {v}");
    }
    assert!(rng_codes(&with(&["scope", "min_isolation"], json!(0))).is_empty());
}

#[test]
fn nonfinite_is_reported_at_any_depth_and_nothing_else_fires() {
    let mut s = template();
    s["scope"]["budget"]["money_usd"] = json!("!nonfinite:inf");
    s["mutations"]["triggers"] =
        json!([{ "id": "T1", "metric": "m", "threshold_e": "!nonfinite:nan" }]);
    let d = rules::check_generic(&s, &v0());
    let nums: Vec<_> = d
        .iter()
        .filter(|x| x.code == "E-NUM-001")
        .map(|x| x.path.as_str())
        .collect();
    assert_eq!(
        nums,
        [
            "mutations.triggers[0].threshold_e",
            "scope.budget.money_usd"
        ]
    );
    assert_eq!(
        d.iter().filter(|x| x.severity == Severity::Error).count(),
        2
    );
}

#[test]
fn type_rules_accept_and_reject_the_right_values() {
    let bad = |path: &[&str], v: Value| -> bool {
        rules::check_generic(&with(path, v), &v0())
            .iter()
            .any(|d| d.code == "E-TYP-001" && d.path == path.join("."))
    };
    assert!(!bad(&["design", "alpha"], json!(1))); // float takes an int
    assert!(!bad(&["design", "alpha"], json!("!nonfinite:nan"))); // E-NUM-001 reports it
    assert!(bad(&["design", "alpha"], json!("x")));
    assert!(bad(&["design", "repeats"], json!(1.5))); // int rejects float
    assert!(bad(&["design", "repeats"], json!(true)));
    assert!(bad(&["study", "test_log"], json!("no")));
    assert!(bad(&["study", "id"], json!(3)));
    assert!(bad(&["design", "selection"], json!([])));
    assert!(bad(&["design", "conditions"], json!({})));
    assert!(!bad(&["design", "conditions"], json!([{"id": "a"}])));
}

#[test]
fn child_checks_skip_when_the_parent_is_missing_or_wrong() {
    let mut s = template();
    s.as_object_mut().unwrap().remove("scope");
    s["design"] = json!("not a table");
    s["outcomes"] = json!({});
    let d = rules::check_generic(&s, &v0());
    let got: Vec<_> = d
        .iter()
        .filter(|x| x.severity == Severity::Error)
        .map(|x| (x.code.as_str(), x.path.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("E-REQ-001", "scope"),
            ("E-TYP-001", "outcomes"),
            ("E-TYP-001", "design"),
        ]
    );
}

#[test]
fn array_elements_are_checked_individually() {
    let mut s = template();
    s["outcomes"] = json!([
        { "id": "a", "unit": "s", "direction": "lower", "registered": true },
        { "id": "b", "unit": "", "direction": "higher", "registered": true },
        { "id": "c", "unit": "s", "direction": "up", "registered": true },
    ]);
    let d = rules::check_generic(&s, &v0());
    assert_eq!(
        codes_paths(&d),
        [
            ("E-OUT-001", "outcomes[1].unit"),
            ("E-OUT-002", "outcomes[2].direction"),
            ("W-LIC-001", "inputs[0].licence.spdx"),
        ]
    );
}

#[test]
fn unknown_keys_report_once_at_the_top_and_inside_array_elements() {
    let mut s = template();
    s["extra"] = json!({ "deep": { "deeper": 1 } });
    s["outcomes"][0]["colour"] = json!("red");
    let d = rules::check_generic(&s, &v0());
    let got: Vec<_> = d
        .iter()
        .filter(|x| x.code == "E-KEY-001")
        .map(|x| x.path.as_str())
        .collect();
    assert_eq!(got, ["extra", "outcomes[0].colour"]);
}

#[test]
fn when_filters_by_mode_and_label() {
    let r = one_rule(
        "when = { mode = \"confirmatory\" }",
        "path = \"study.id\"\ncheck = \"non-empty\"",
    );
    let mut s = template();
    s["study"]["id"] = json!("");
    s["study"]["mode"] = json!("exploratory");
    assert!(rules::check_generic(&s, &r).is_empty());
    s["study"]["mode"] = json!("confirmatory");
    assert_eq!(
        codes_paths(&rules::check_generic(&s, &r)),
        [("X-TST-001", "study.id")]
    );

    let r = one_rule(
        "when = { mode = \"confirmatory\", label = \"signal\" }",
        "path = \"study.id\"\ncheck = \"non-empty\"",
    );
    assert!(rules::check_generic(&s, &r).len() == 1);
    s["study"]["label"] = json!("confirmed");
    assert!(rules::check_generic(&s, &r).is_empty());
    s["study"]["label"] = json!("signal");
    s["study"]["mode"] = json!("exploratory");
    assert!(rules::check_generic(&s, &r).is_empty());
}

#[test]
fn warning_severity_is_kept() {
    let r = local_rules(&format!(
        "[[rule]]\n{}\n[[rule.target]]\npath = \"study.id\"\ncheck = \"required\"\n",
        HEAD.replace("\"error\"", "\"warning\"")
    ));
    let d = rules::check_generic(&json!({ "study": {} }), &r);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].severity, Severity::Warning);
}

#[test]
fn named_checks_are_skipped_by_the_generic_checker() {
    let r = one_rule("", "path = \"study.id\"\ncheck = \"named:anything\"");
    assert!(matches!(&r[0].check, Check::Named(n) if n == "anything"));
    assert!(rules::check_generic(&json!({}), &r).is_empty());
}

#[test]
fn enum_and_format_skip_missing_and_wrong_typed_values() {
    let r = one_rule("", "path = \"a\"\ncheck = \"enum\"\nvalues = [\"x\"]");
    assert!(rules::check_generic(&json!({}), &r).is_empty());
    assert!(rules::check_generic(&json!({"a": 1}), &r).is_empty());
    assert!(rules::check_generic(&json!({"a": "x"}), &r).is_empty());
    assert_eq!(rules::check_generic(&json!({"a": "y"}), &r).len(), 1);
    let r = one_rule("", "path = \"a\"\ncheck = \"format\"\npattern = \"^x$\"");
    assert!(rules::check_generic(&json!({}), &r).is_empty());
    assert!(rules::check_generic(&json!({"a": 1}), &r).is_empty());
    assert!(rules::check_generic(&json!({"a": "x"}), &r).is_empty());
    assert_eq!(rules::check_generic(&json!({"a": "xx"}), &r).len(), 1);
}

#[test]
fn non_empty_fails_on_missing_empty_string_and_empty_array() {
    let r = one_rule("", "path = \"a\"\ncheck = \"non-empty\"");
    for bad in [json!({}), json!({"a": ""}), json!({"a": []})] {
        assert_eq!(rules::check_generic(&bad, &r).len(), 1, "{bad}");
    }
    for ok in [
        json!({"a": " "}),
        json!({"a": [1]}),
        json!({"a": 0}),
        json!({"a": false}),
    ] {
        assert!(rules::check_generic(&ok, &r).is_empty(), "{ok}");
    }
}

#[test]
fn interval_notation_parses_every_bound_kind() {
    let range = |iv: &str| -> Check {
        one_rule(
            "",
            &format!("path = \"a\"\ncheck = \"range\"\ninterval = \"{iv}\""),
        )
        .remove(0)
        .check
    };
    let ok = |iv: &str, x: f64| {
        rules::check_generic(
            &json!({ "a": x }),
            &one_rule(
                "",
                &format!("path = \"a\"\ncheck = \"range\"\ninterval = \"{iv}\""),
            ),
        )
        .is_empty()
    };
    assert!(matches!(range("(0, 1]"), Check::Range { .. }));
    assert!(!ok("(0, 1]", 0.0) && ok("(0, 1]", 1.0) && !ok("(0, 1]", 1.5));
    assert!(ok("[0, 1)", 0.0) && !ok("[0, 1)", 1.0));
    assert!(ok("[1, inf)", 1e300) && !ok("[1, inf)", 0.9));
    assert!(ok("(-inf, 5]", -1e300) && !ok("(-inf, 5]", 5.1));
}

#[test]
fn bad_rule_files_are_errors_not_panics() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("r.toml");
    let try_load = |text: &str| {
        std::fs::write(&f, text).unwrap();
        rules::load(&f)
    };
    assert!(rules::load(&dir.path().join("missing.toml")).is_err());
    assert!(try_load("not toml [").is_err());
    let rule = |t: &str| format!("[[rule]]\n{HEAD}[[rule.target]]\n{t}\n");
    assert!(try_load(&rule("path = \"a\"\ncheck = \"type\"\nkind = \"blob\"")).is_err());
    for t in [
        "path = \"a\"\ncheck = \"nope\"",
        "path = \"a\"\ncheck = \"type\"",
        "path = \"a\"\ncheck = \"enum\"",
        "path = \"a\"\ncheck = \"range\"",
        "path = \"a\"\ncheck = \"range\"\ninterval = \"1, 2\"",
        "path = \"a\"\ncheck = \"range\"\ninterval = \"[2, 1]\"",
        "path = \"a\"\ncheck = \"format\"",
        "path = \"a\"\ncheck = \"format\"\npattern = \"(\"",
        "path = \"a\"\ncheck = \"named:\"",
    ] {
        let e = try_load(&rule(t)).unwrap_err().to_string();
        assert!(e.contains("X-TST-001"), "{t}: {e}");
    }
}

#[test]
fn one_code_reports_a_path_once_even_when_two_targets_fail() {
    // An empty direction fails both `non-empty` and `enum` under E-OUT-002.
    let mut s = template();
    s["outcomes"][0]["direction"] = json!("");
    let d = rules::check_generic(&s, &v0());
    let hits: Vec<_> = d.iter().filter(|x| x.code == "E-OUT-002").collect();
    assert_eq!(hits.len(), 1);
}
