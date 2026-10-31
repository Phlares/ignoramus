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

/// (code, JSON pointer, new value or `None` to remove, expected path).
type Fault = (&'static str, &'static str, Option<Value>, &'static str);

fn faults() -> Vec<Fault> {
    let nan = || Some(json!("!nonfinite:nan"));
    vec![
        ("E-REQ-001", "/study/mode", None, "study.mode"),
        (
            "E-TYP-001",
            "/design/repeats",
            Some(json!("one")),
            "design.repeats",
        ),
        (
            "E-ENM-001",
            "/study/mode",
            Some(json!("sometimes")),
            "study.mode",
        ),
        (
            "E-RNG-001",
            "/design/power",
            Some(json!(0.4)),
            "design.power",
        ),
        ("E-FMT-001", "/study/id", Some(json!("étude-1")), "study.id"),
        (
            "E-RNG-001",
            "/scope/budget/cpu_hours",
            Some(json!(-0.5)),
            "scope.budget.cpu_hours",
        ),
        (
            "E-RNG-001",
            "/scope/budget/wall_hours",
            Some(json!(-0.5)),
            "scope.budget.wall_hours",
        ),
        (
            "E-RNG-001",
            "/scope/budget/money_usd",
            Some(json!(-0.5)),
            "scope.budget.money_usd",
        ),
        (
            "E-FMT-001",
            "/design/conditions/1/id",
            Some(json!("Arm A")),
            "design.conditions[1].id",
        ),
        (
            "E-FMT-003",
            "/inputs/0/sha256",
            Some(json!("ABC")),
            "inputs[0].sha256",
        ),
        (
            "E-NUM-001",
            "/design/row_seconds",
            nan(),
            "design.row_seconds",
        ),
        (
            "E-KEY-001",
            "/study/surprise",
            Some(json!(1)),
            "study.surprise",
        ),
        (
            "E-OUT-001",
            "/outcomes/0/unit",
            Some(json!("")),
            "outcomes[0].unit",
        ),
        ("E-OUT-001", "/outcomes/0/unit", None, "outcomes[0].unit"),
        (
            "E-OUT-002",
            "/outcomes/0/direction",
            Some(json!("")),
            "outcomes[0].direction",
        ),
        (
            "E-OUT-002",
            "/outcomes/0/direction",
            Some(json!("sideways")),
            "outcomes[0].direction",
        ),
        (
            "E-OUT-002",
            "/outcomes/0/direction",
            None,
            "outcomes[0].direction",
        ),
        (
            "E-REQ-001",
            "/outcomes/0/measure",
            None,
            "outcomes[0].measure",
        ),
        (
            "E-REQ-001",
            "/outcomes/0/paired",
            None,
            "outcomes[0].paired",
        ),
        (
            "E-TYP-001",
            "/outcomes/0/measure",
            Some(json!(1)),
            "outcomes[0].measure",
        ),
        (
            "E-TYP-001",
            "/outcomes/0/paired",
            Some(json!(true)),
            "outcomes[0].paired",
        ),
        (
            "E-ENM-001",
            "/outcomes/0/paired",
            Some(json!("ratio")),
            "outcomes[0].paired",
        ),
        (
            "W-LIC-001",
            "/inputs/0/licence/spdx",
            Some(json!("")),
            "inputs[0].licence.spdx",
        ),
        (
            "E-REF-001",
            "/design/conditions",
            Some(json!([])),
            "design.conditions",
        ),
        ("E-TYP-001", "/outcomes", Some(json!([1])), "outcomes[0]"),
        (
            "E-TYP-001",
            "/question/hypotheses",
            Some(json!([1])),
            "question.hypotheses[0]",
        ),
        (
            "E-TYP-001",
            "/scope/end_of_run/follow_on",
            Some(json!([1])),
            "scope.end_of_run.follow_on[0]",
        ),
        (
            "E-TYP-001",
            "/design/conditions",
            Some(json!([1])),
            "design.conditions[0]",
        ),
        ("E-TYP-001", "/inputs", Some(json!([1])), "inputs[0]"),
        (
            "E-TYP-001",
            "/inputs/0/licence/creators",
            Some(json!([1])),
            "inputs[0].licence.creators[0]",
        ),
        (
            "E-TYP-001",
            "/mutations/operators",
            Some(json!([1])),
            "mutations.operators[0]",
        ),
        (
            "E-TYP-001",
            "/mutations/triggers",
            Some(json!([1])),
            "mutations.triggers[0]",
        ),
    ]
}

fn apply(spec: &mut Value, ptr: &str, v: Option<Value>) {
    let (parent, key) = ptr.rsplit_once('/').unwrap();
    match (spec.pointer_mut(parent).unwrap(), v) {
        (Value::Object(m), Some(v)) => {
            m.insert(key.into(), v);
        }
        (Value::Object(m), None) => {
            m.remove(key).unwrap();
        }
        (Value::Array(a), Some(v)) => a[key.parse::<usize>().unwrap()] = v,
        _ => panic!("cannot apply {ptr}"),
    }
}

fn faulted(rows: &[Fault]) -> Value {
    let mut s = template();
    for (_, ptr, v, _) in rows {
        apply(&mut s, ptr, v.clone());
    }
    s
}

#[test]
fn each_fault_row_yields_its_code_and_path_alone() {
    let rules = v0();
    for row in faults() {
        let (code, _, _, path) = &row;
        let d = rules::check_generic(&faulted(std::slice::from_ref(&row)), &rules);
        // The template itself carries both warnings on each input; drop them unless the row is
        // about that warning at that path.
        let d: Vec<_> = d
            .into_iter()
            .filter(|x| !x.code.starts_with("W-") || (x.code == *code && x.path == *path))
            .collect();
        assert_eq!(codes_paths(&d), [(*code, *path)], "{row:?}");
    }
}

#[test]
fn templates_have_no_generic_errors_and_no_unknown_keys() {
    let rules = v0();
    for t in ["numeric-ablation", "planted-spawn"] {
        let d = rules::check_generic(&spec_at(&format!("../../templates/{t}.toml")), &rules);
        assert_eq!(
            codes_paths(&d),
            [
                ("W-LIC-001", "inputs[0].licence.spdx"),
                ("W-LIC-001", "inputs[1].licence.spdx"),
                ("W-HSH-001", "inputs[0].sha256"),
                ("W-HSH-001", "inputs[1].sha256"),
            ]
        );
        assert!(d.iter().all(|x| x.severity == Severity::Warning));
    }
}

fn three_faults() -> Value {
    let rows = faults();
    let pick = |i: usize| rows[i].clone();
    // study.mode removed, power 0.4, study.id non-ASCII.
    faulted(&[pick(0), pick(3), pick(4)])
}

#[test]
fn all_faults_reported_at_once_in_rule_file_order() {
    let d = rules::check_generic(&three_faults(), &v0());
    assert_eq!(
        codes_paths(&d),
        [
            ("E-REQ-001", "study.mode"),
            ("E-RNG-001", "design.power"),
            ("E-FMT-001", "study.id"),
            ("W-LIC-001", "inputs[0].licence.spdx"),
            ("W-LIC-001", "inputs[1].licence.spdx"),
            ("W-HSH-001", "inputs[0].sha256"),
            ("W-HSH-001", "inputs[1].sha256"),
        ]
    );
}

#[test]
fn diagnostic_carries_rule_text_sources_and_doc() {
    let mut s = template();
    apply(&mut s, "/outcomes/0/unit", Some(json!("")));
    let d = rules::check_generic(&s, &v0());
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
    let d = rules::check_generic(&three_faults(), &v0());
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
fn missing_licence_id_gives_the_schema_error_then_the_warning() {
    let mut s = template();
    apply(&mut s, "/inputs/0/licence/spdx", None);
    let d = rules::check_generic(&s, &v0());
    assert_eq!(
        codes_paths(&d),
        [
            ("E-REQ-001", "inputs[0].licence.spdx"),
            ("W-LIC-001", "inputs[0].licence.spdx"),
            ("W-LIC-001", "inputs[1].licence.spdx"),
            ("W-HSH-001", "inputs[0].sha256"),
            ("W-HSH-001", "inputs[1].sha256"),
        ]
    );
}

/// The templates with extra elements, so every `[*]` path has nodes at index 0 and 1.
fn fat_templates() -> Vec<Value> {
    ["numeric-ablation", "planted-spawn"]
        .iter()
        .map(|t| {
            let mut s = spec_at(&format!("../../templates/{t}.toml"));
            let mut second = s["outcomes"][0].clone();
            second["id"] = json!("second");
            second["measure"] = json!("second-measure");
            s["inputs"][0]["licence"]["creators"] = json!(["x", "y"]);
            s["pins"]["deterministic"] = json!(true);
            s["design"]["row_memory_mib"] = json!(1024);
            s["pins"]["deviations"] = json!([
                { "from": "pilot", "reason": "r", "timing": "before-collection" },
                { "from": "early", "reason": "r", "timing": "after-results" },
            ]);
            s["mutations"]["operators"] = json!([
                { "name": "o1", "type": "int", "range": [1, 2], "acts_on": "a" },
                { "name": "o2", "type": "int", "range": [1, 2], "acts_on": "a" },
            ]);
            s["mutations"]["triggers"] = json!([
                { "id": "t1", "metric": "ppl-ratio", "threshold_e": 1.0 },
                { "id": "t2", "metric": "ppl-ratio", "threshold_e": 1.0 },
            ]);
            s["outcomes"].as_array_mut().unwrap().push(second);
            s["design"]["conditions"][1]["params"] =
                json!({ "bits": 4, "mode": "fast", "chosen": { "from": "chooser" } });
            s["design"]["selection"] =
                json!({ "metric": "ppl-ratio", "conditions": ["a", "b"], "rule": "best-mean" });
            s["scope"]["end_of_run"]["follow_on"] = json!([
                { "when": "H1.pass", "spec": "follow/a.toml" },
                { "when": "H1.pass", "spec": "follow/b.toml" },
            ]);
            // One hypothesis per claim shape, so every claim and statistic key has a node.
            let h = &mut s["question"]["hypotheses"];
            h[0]["contrast"] = json!({ "arm-a": 1.0, "selected": -1.0 });
            h[0]["strata"] = json!(["fiction", "code"]);
            h[0]["power_at"] = json!(0.5);
            h[0]["estimates"] = json!({ "from": "pilot" });
            h[0]["alpha_share"] = json!(0.5);
            for (i, claim, stat) in [
                (0, json!({ "below": 1.0 }), json!({ "mean": [-5.0, 5.0] })),
                (1, json!({ "above": 0.0 }), json!({ "quantile": 0.5 })),
                (
                    2,
                    json!({ "within": [-1.0, 1.0] }),
                    json!({ "mean": [-5.0, 5.0] }),
                ),
            ] {
                if i > 0 {
                    let mut next = h[0].clone();
                    next["id"] = json!(format!("H{}", i + 1));
                    h.as_array_mut().unwrap().push(next);
                }
                h[i]["claim"] = claim;
                h[i]["statistic"] = stat;
            }
            s
        })
        .collect()
}

/// Concrete paths a pattern selects in `v`, counting only nodes that exist. A `*` segment
/// selects every key of a table.
fn expand(v: &Value, pattern: &str) -> Vec<String> {
    let join = |p: &str, name: &str| {
        if p.is_empty() {
            name.to_string()
        } else {
            format!("{p}.{name}")
        }
    };
    let mut cur = vec![(String::new(), v)];
    for seg in pattern.split('.') {
        let (name, each) = match seg.strip_suffix("[*]") {
            Some(n) => (n, true),
            None => (seg, false),
        };
        let mut next = vec![];
        for (p, node) in cur {
            if seg == "*" {
                let keys = node.as_object().into_iter().flatten();
                next.extend(keys.map(|(k, c)| (join(&p, k), c)));
                continue;
            }
            let Some(child) = node.get(name) else {
                continue;
            };
            let base = join(&p, name);
            match (each, child) {
                (false, c) => next.push((base, c)),
                (true, Value::Array(a)) => {
                    next.extend(
                        a.iter()
                            .enumerate()
                            .map(|(i, e)| (format!("{base}[{i}]"), e)),
                    );
                }
                _ => {}
            }
        }
        cur = next;
    }
    cur.into_iter().map(|(p, _)| p).collect()
}

fn remove_at(v: &mut Value, path: &str) {
    let (parent, last) = path.rsplit_once('.').unwrap_or(("", path));
    let mut cur = v;
    for seg in parent.split('.').filter(|s| !s.is_empty()) {
        let (name, idx) = match seg.split_once('[') {
            Some((n, i)) => (n, Some(i.trim_end_matches(']').parse::<usize>().unwrap())),
            None => (seg, None),
        };
        cur = &mut cur[name];
        if let Some(i) = idx {
            cur = &mut cur[i];
        }
    }
    let name = last.split('[').next().unwrap();
    cur.as_object_mut().unwrap().remove(name).unwrap();
}

#[test]
fn removing_any_required_node_reports_it_at_its_path() {
    let rules = v0();
    let fat = fat_templates();
    for r in rules.iter().filter(|r| matches!(r.check, Check::Required)) {
        let mut hit = 0;
        for t in &fat {
            for path in expand(t, &r.path) {
                // Array elements themselves are not keys; only keyed nodes are removed.
                if path.ends_with(']') {
                    continue;
                }
                let mut s = t.clone();
                remove_at(&mut s, &path);
                let d = rules::check_generic(&s, &rules);
                assert!(
                    d.iter().any(|x| x.code == r.code && x.path == path),
                    "{} {path}: {d:?}",
                    r.code
                );
                hit += 1;
            }
        }
        assert!(hit > 0, "required target selects nothing: {}", r.path);
    }
}

#[test]
fn every_non_wildcard_rule_path_selects_a_node_in_some_template() {
    let fat = fat_templates();
    for r in v0() {
        if r.path == "**" || matches!(r.check, Check::Named(_) | Check::Documented) {
            continue;
        }
        let n: usize = fat.iter().map(|t| expand(t, &r.path).len()).sum();
        assert!(
            n > 0,
            "{} {} ({}) selects nothing",
            r.code,
            r.path,
            r.check.name()
        );
    }
}

#[test]
fn indexed_paths_cover_second_elements() {
    let rules = v0();
    let mut s = fat_templates().remove(0);
    s["outcomes"][1]["unit"] = json!("");
    s["design"]["conditions"][1]["id"] = json!("B");
    let d = rules::check_generic(&s, &rules);
    assert_eq!(
        codes_paths(&d),
        [
            ("E-FMT-001", "design.conditions[1].id"),
            ("E-OUT-001", "outcomes[1].unit"),
            ("W-LIC-001", "inputs[0].licence.spdx"),
            ("W-LIC-001", "inputs[1].licence.spdx"),
            ("W-HSH-001", "inputs[0].sha256"),
            ("W-HSH-001", "inputs[1].sha256"),
        ]
    );
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

/// The key of a unique check: two unique checks on one array differ by it.
fn unique_key(c: &Check) -> Option<String> {
    match c {
        Check::Unique(k) => k.clone(),
        _ => None,
    }
}

#[test]
fn no_duplicate_path_and_kind_pairs() {
    let mut seen = HashSet::new();
    for r in v0()
        .iter()
        .filter(|r| !matches!(r.check, Check::Documented))
    {
        assert!(
            seen.insert((
                r.path.clone(),
                unique_key(&r.check).unwrap_or_default(),
                r.check.name()
            )),
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
    // A key under a `*` target, such as a contrast's condition id, is typed by that target.
    let typed = |p: &str| {
        types.contains(p)
            || p.rsplit_once('.')
                .is_some_and(|(parent, _)| types.contains(&format!("{parent}.*")))
    };
    for t in ["numeric-ablation", "planted-spawn"] {
        let mut all = vec![];
        paths(&spec_at(&format!("../../templates/{t}.toml")), "", &mut all);
        let missing: Vec<_> = all.iter().filter(|p| !typed(p)).collect();
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
        "design.power",
        "design.repeats",
        "mutations.depth_cap",
        "scope.min_isolation",
        "access.blind",
        "outcomes[*].unit",
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
    let cases: [(&[&str], Value, bool); 10] = [
        (&["design", "power"], json!(0.4999), true),
        (&["design", "power"], json!(0.5), false),
        (&["design", "power"], json!(0.999), false),
        (&["design", "power"], json!(0.9991), true),
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
fn a_plan_cannot_set_its_own_alpha() {
    // The level comes from the box-wide stream, so design.alpha is an unknown key.
    let d = rules::check_generic(&with(&["design", "alpha"], json!(0.005)), &v0());
    let errors: Vec<_> = (d.iter())
        .filter(|x| x.severity == Severity::Error)
        .map(|x| (x.code.as_str(), x.path.as_str()))
        .collect();
    assert_eq!(errors, [("E-KEY-001", "design.alpha")]);
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
    assert!(!bad(&["design", "power"], json!(1))); // float takes an int
    assert!(!bad(&["design", "power"], json!("!nonfinite:nan"))); // E-NUM-001 reports it
    assert!(bad(&["design", "power"], json!("x")));
    assert!(bad(&["design", "repeats"], json!(1.5))); // int rejects float
    assert!(bad(&["design", "repeats"], json!(true)));
    assert!(!bad(&["design", "repeats"], json!(-1))); // negative and
    assert!(!bad(&["design", "repeats"], json!(u64::MAX))); // > i64::MAX are still ints
    assert!(bad(&["study", "test_log"], json!("no")));
    assert!(bad(&["pins", "deterministic"], json!("yes")));
    assert!(!bad(&["pins", "deterministic"], json!(true)));
    assert!(bad(&["study", "id"], json!(3)));
    assert!(bad(&["design", "pick"], json!([])));
    assert!(bad(&["design", "conditions"], json!({})));
    assert!(!bad(&["design", "conditions"], json!([{"id": "a"}])));
}

#[test]
fn every_paired_rule_passes_the_enum_check() {
    let enum_hit = |rule: &str| -> bool {
        let mut s = template();
        s["outcomes"][0]["paired"] = json!(rule);
        rules::check_generic(&s, &v0())
            .iter()
            .any(|d| d.code == "E-ENM-001" && d.path == "outcomes[0].paired")
    };
    for rule in ["difference", "percent", "log-ratio"] {
        assert!(!enum_hit(rule), "{rule}");
    }
    assert!(enum_hit("ratio"));
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
        { "id": "a", "unit": "s", "direction": "lower", "measure": "m1", "paired": "percent", "registered": true },
        { "id": "b", "unit": "", "direction": "higher", "measure": "m2", "paired": "percent", "registered": true },
        { "id": "c", "unit": "s", "direction": "up", "measure": "m3", "paired": "percent", "registered": true },
    ]);
    let d = rules::check_generic(&s, &v0());
    assert_eq!(
        codes_paths(&d),
        [
            ("E-OUT-001", "outcomes[1].unit"),
            ("E-OUT-002", "outcomes[2].direction"),
            ("W-LIC-001", "inputs[0].licence.spdx"),
            ("W-LIC-001", "inputs[1].licence.spdx"),
            ("W-HSH-001", "inputs[0].sha256"),
            ("W-HSH-001", "inputs[1].sha256"),
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
fn a_star_segment_selects_every_key_and_known_key_accepts_them() {
    let r = local_rules(&format!(
        "[[rule]]\n{HEAD}target = [\n\
         {{ path = \"a\", check = \"type\", kind = \"table\" }},\n\
         {{ path = \"a.*\", check = \"type\", kind = \"int\" }},\n\
         {{ path = \"b\", check = \"type\", kind = \"table\" }},\n\
         {{ path = \"b.*\", check = \"type\", kind = \"any\" }},\n\
         {{ path = \"**\", check = \"known-key\" }},\n]\n"
    ));
    let s = json!({
        "a": { "x": 1, "y": "two", "Pasted Secret": "three", "p.q": "four" },
        "b": { "t": { "deep": 1 }, "u": [1, "x"], "v": true },
        "c": 1,
    });
    let got: Vec<_> = rules::check_generic(&s, &r)
        .into_iter()
        .map(|d| d.path)
        .collect();
    // `a.*` types every key of `a`, naming an unsafe key `<key>`; `b.*` (any) checks no
    // type and keeps known-key out of its values. A key with a dot is never a `*` key: only
    // known-key reports it, not the type rule.
    assert_eq!(got, ["a.<key>", "a.y", "a.<key>", "c"]);
}

#[test]
fn a_key_holding_a_dot_or_bracket_is_unknown_and_never_printed() {
    let mut s = template();
    s["design.alpha"] = json!(0.1);
    s["design"]["units.per_cluster"] = json!(1.0);
    s["outcomes"][0]["a[0]"] = json!(1);
    let d = rules::check_generic(&s, &v0());
    let got: Vec<_> = d
        .iter()
        .filter(|x| x.code == "E-KEY-001")
        .map(|x| x.path.as_str())
        .collect();
    assert_eq!(got, ["design.<key>", "<key>", "outcomes[0].<key>"]);
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
fn a_confirmatory_plan_needs_a_hypothesis() {
    let cnf006 = |mode: &str, hypotheses: Option<Value>| {
        let mut s = template();
        s["study"]["mode"] = json!(mode);
        match hypotheses {
            Some(h) => s["question"]["hypotheses"] = h,
            None => {
                s["question"].as_object_mut().unwrap().remove("hypotheses");
            }
        }
        let d = rules::check_generic(&s, &v0());
        d.into_iter()
            .filter(|x| x.code == "E-CNF-006")
            .map(|x| x.path)
            .collect::<Vec<_>>()
    };
    // No hypothesis has nothing to test, and its slot could never give a discovery.
    assert_eq!(
        cnf006("confirmatory", Some(json!([]))),
        ["question.hypotheses"]
    );
    assert_eq!(cnf006("confirmatory", None), ["question.hypotheses"]);
    assert!(cnf006("exploratory", Some(json!([]))).is_empty());
    assert!(
        cnf006(
            "confirmatory",
            Some(template()["question"]["hypotheses"].clone())
        )
        .is_empty()
    );
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
    let r = one_rule("", "path = \"study.id\"\ncheck = \"named:power-met\"");
    assert!(matches!(&r[0].check, Check::Named(n) if n == "power-met"));
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

fn rule_text(target: &str) -> String {
    format!("[[rule]]\n{HEAD}[[rule.target]]\n{target}\n")
}

fn load_text(text: &str) -> Result<Vec<Rule>, rules::RulesError> {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("r.toml");
    std::fs::write(&f, text).unwrap();
    rules::load(&f)
}

#[test]
fn load_errors_name_the_code_the_value_and_the_line() {
    let ok_target = "path = \"a\"\ncheck = \"required\"";
    let cases = [
        // (text, strings the message must contain)
        (
            rule_text(ok_target).replace("severity = \"error\"", "severity = \"fatal\""),
            vec!["X-TST-001", "fatal", "line 3"],
        ),
        (
            rule_text("path = \"a\"\ncheck = \"type\"\nkind = \"blob\""),
            vec!["X-TST-001", "blob", "line 11"],
        ),
        (
            rule_text(ok_target).replace("why = \"w\"", "why = \"w\"\nextra = 1"),
            vec!["X-TST-001", "extra", "line 7"],
        ),
        (
            rule_text(ok_target).replace("why = \"w\"\n", ""),
            vec!["X-TST-001", "why"],
        ),
    ];
    for (text, needles) in cases {
        let e = load_text(&text).unwrap_err().to_string();
        for n in needles {
            assert!(e.contains(n), "missing {n:?} in: {e}");
        }
    }
    let e = load_text("not toml [").unwrap_err().to_string();
    assert!(e.contains("line 1"), "{e}");
    assert!(rules::load("no/such/rules.toml".as_ref()).is_err());
}

#[test]
fn bad_targets_are_rejected_with_the_code_and_path() {
    let bad = [
        "path = \"a\"\ncheck = \"nope\"",
        "path = \"a\"\ncheck = \"type\"",
        "path = \"a\"\ncheck = \"enum\"",
        "path = \"a\"\ncheck = \"range\"",
        "path = \"a\"\ncheck = \"range\"\ninterval = \"1, 2\"",
        "path = \"a\"\ncheck = \"format\"",
        "path = \"a\"\ncheck = \"format\"\npattern = \"(\"",
        "path = \"a\"\ncheck = \"named:\"",
        // Params that do not belong to the check.
        "path = \"a\"\ncheck = \"range\"\ninterval = \"[0, 1]\"\nvalues = [\"x\"]",
        "path = \"a\"\ncheck = \"required\"\nkind = \"int\"",
        "path = \"a\"\ncheck = \"enum\"\nvalues = [\"x\"]\npattern = \"x\"",
        "path = \"a\"\ncheck = \"finite\"\ninterval = \"[0, 1]\"",
        "path = \"a\"\ncheck = \"named:x\"\nkind = \"int\"",
        "path = \"a\"\ncheck = \"required\"\nkey = \"id\"",
        "path = \"a\"\ncheck = \"unique\"\nvalues = [\"x\"]",
    ];
    for t in bad {
        let e = load_text(&rule_text(t)).unwrap_err().to_string();
        assert!(e.contains("rule X-TST-001 at a: "), "{t}: {e}");
    }
    let e = load_text(&rule_text(
        "path = \"a\"
check = \"named:nope\"",
    ))
    .unwrap_err()
    .to_string();
    assert!(e.contains("unknown named check \"nope\""), "{e}");
}

#[test]
fn interval_syntax_is_strict() {
    let interval = |iv: &str| {
        load_text(&rule_text(&format!(
            "path = \"a\"\ncheck = \"range\"\ninterval = \"{iv}\""
        )))
        .is_ok()
    };
    for ok in [
        "[0, 1]",
        "(0, 1)",
        "[1, 1]",
        "[1, inf)",
        "(-inf, 5]",
        " [0,1] ",
    ] {
        assert!(interval(ok), "{ok}");
    }
    for bad in [
        "0, 1]",
        "[0, 1",
        "0, 1",
        "{0, 1}",
        "[0 1]",
        "[0, 1, 2]",
        "[a, 1]",
        "[nan, 1]",
        "[0, nan]",
        "[2, 1]",
        "(1, 1]",
        "[1, 1)",
        "(1, 1)",
        "[infinity, 5]",
        "[0, infinity]",
        "[0, 1}",
        "{0, 1]",
        "",
        "[",
        "[]",
    ] {
        assert!(!interval(bad), "{bad}");
    }
}

#[test]
fn budget_values_of_zero_pass_the_range_and_negatives_do_not() {
    for k in ["cpu_hours", "wall_hours", "money_usd"] {
        let zero = with(&["scope", "budget", k], json!(0.0));
        assert!(rng_codes(&zero).is_empty(), "{k}");
        let neg = with(&["scope", "budget", k], json!(-1e-9));
        assert_eq!(rng_codes(&neg), [format!("scope.budget.{k}")]);
    }
}

#[test]
fn cost_inputs_are_range_checked_at_their_edges() {
    let trig = |e: Value| json!([{ "id": "T1", "metric": "m", "threshold_e": e }]);
    let cases: Vec<(Vec<&str>, Value, Vec<String>)> = vec![
        (
            vec!["design", "row_seconds"],
            json!(0.0),
            vec!["design.row_seconds".into()],
        ),
        (
            vec!["design", "row_seconds"],
            json!(-1.0),
            vec!["design.row_seconds".into()],
        ),
        (vec!["design", "row_seconds"], json!(1e-9), vec![]),
        (
            vec!["design", "units_per_cluster"],
            json!(0),
            vec!["design.units_per_cluster".into()],
        ),
        (
            vec!["design", "units_per_cluster"],
            json!(-3),
            vec!["design.units_per_cluster".into()],
        ),
        (vec!["design", "units_per_cluster"], json!(1), vec![]),
        (
            vec!["mutations", "depth_cap"],
            json!(-1),
            vec!["mutations.depth_cap".into()],
        ),
        (vec!["mutations", "depth_cap"], json!(0), vec![]),
        (vec!["mutations", "depth_cap"], json!(255), vec![]),
        (
            vec!["mutations", "depth_cap"],
            json!(256),
            vec!["mutations.depth_cap".into()],
        ),
        (vec!["design", "repeats"], json!(4294967295u32), vec![]),
        (
            vec!["design", "repeats"],
            json!(4294967296u64),
            vec!["design.repeats".into()],
        ),
        (
            vec!["design", "units_per_cluster"],
            json!(4294967295u32),
            vec![],
        ),
        (
            vec!["design", "units_per_cluster"],
            json!(4294967296u64),
            vec!["design.units_per_cluster".into()],
        ),
        (
            vec!["mutations", "triggers"],
            trig(json!(0.0)),
            vec!["mutations.triggers[0].threshold_e".into()],
        ),
        (
            vec!["mutations", "triggers"],
            trig(json!(-1.0)),
            vec!["mutations.triggers[0].threshold_e".into()],
        ),
        (vec!["mutations", "triggers"], trig(json!(1e-9)), vec![]),
    ];
    for (path, v, want) in cases {
        assert_eq!(rng_codes(&with(&path, v.clone())), want, "{path:?} = {v}");
    }
}

#[test]
fn a_row_memory_is_a_whole_number_of_mib_from_1_to_1048576() {
    let at = |v: Value| with(&["design", "row_memory_mib"], v);
    let errors = |s: &Value| -> Vec<(String, String)> {
        (rules::check_generic(s, &v0()).into_iter())
            .filter(|d| d.severity == Severity::Error)
            .map(|d| (d.code, d.path))
            .collect()
    };
    let here = |code: &str| vec![(code.to_string(), "design.row_memory_mib".to_string())];
    for (v, want) in [
        (json!(0), here("E-RNG-001")),
        (json!(-1), here("E-RNG-001")),
        (json!(1), vec![]),
        (json!(1048576), vec![]),
        (json!(1048577), here("E-RNG-001")),
        (json!("lots"), here("E-TYP-001")),
        (json!(1.5), here("E-TYP-001")),
    ] {
        assert_eq!(errors(&at(v.clone())), want, "{v}");
    }
    // Absent, no cap: nothing to report.
    assert!(template()["design"].get("row_memory_mib").is_none());
    assert_eq!(errors(&template()), vec![]);
}

#[test]
fn an_unsafe_key_renders_as_a_placeholder_in_every_output() {
    let token = format!("AKIA{}", "ABCDEFGHIJKLMNOP");
    let mut s = template();
    s["study"][&token] = json!("x");
    let d = rules::check_generic(&s, &v0());
    let key: Vec<_> = d.iter().filter(|x| x.code == "E-KEY-001").collect();
    assert_eq!(key.len(), 1);
    assert_eq!(key[0].path, "study.<key>");
    for shown in [render_human(&d), render_json(&d), format!("{d:?}")] {
        assert!(!shown.contains(&token), "{shown}");
    }
}

#[test]
fn key_safety_boundary_is_one_to_forty_of_lowercase_digit_underscore_hyphen() {
    let path_of = |key: &str| {
        let mut s = template();
        s["study"][key] = json!(1);
        let d = rules::check_generic(&s, &v0());
        d.into_iter().find(|x| x.code == "E-KEY-001").unwrap().path
    };
    let ok = "a0_-".repeat(10);
    assert_eq!(path_of(&ok), format!("study.{ok}"));
    assert_eq!(path_of(&format!("{ok}a")), "study.<key>");
    assert_eq!(path_of("Ab"), "study.<key>");
    assert_eq!(path_of("a.b"), "study.<key>");
    assert_eq!(path_of(""), "study.<key>");
    assert_eq!(path_of("\u{e9}"), "study.<key>");
}

#[test]
fn a_rule_with_no_targets_is_documented_only_and_checks_nothing() {
    let text = format!(
        "[[rule]]
{HEAD}
check = \"documented\"
"
    );
    let rules = local_rules(&text);
    assert_eq!(rules.len(), 1);
    assert!(matches!(rules[0].check, Check::Documented));
    assert_eq!(rules[0].check.name(), "documented");
    assert!(rules::check_generic(&serde_json::json!({}), &rules).is_empty());
}

#[test]
fn an_empty_target_list_without_the_marker_is_a_load_error() {
    let dir = tempfile::tempdir().unwrap();
    for extra in [
        "target = []",
        "check = \"documented\"
[[rule.target]]
path = \"a\"
check = \"required\"",
        "",
    ] {
        let file = dir.path().join("r.toml");
        std::fs::write(
            &file,
            format!(
                "[[rule]]
{HEAD}{extra}
"
            ),
        )
        .unwrap();
        assert!(rules::load(&file).is_err(), "{extra}");
    }
}

#[test]
fn the_documented_codes_are_exactly_the_register_refusals() {
    let mut codes: Vec<&str> = rules::builtin()
        .iter()
        .filter(|r| matches!(r.check, Check::Documented))
        .map(|r| r.code.as_str())
        .collect();
    codes.sort_unstable();
    assert_eq!(codes, ["E-REG-001", "E-REG-002", "E-REG-003"]);
}

#[test]
fn unique_reports_each_repeat_at_its_element_and_skips_non_strings() {
    let r = one_rule(
        "",
        "path = \"a\"
check = \"unique\"",
    );
    for ok in [
        json!({}),
        json!({"a": "x"}),
        json!({"a": []}),
        json!({"a": ["x", "y"]}),
        json!({"a": [1, 1]}),
    ] {
        assert!(rules::check_generic(&ok, &r).is_empty(), "{ok}");
    }
    let d = rules::check_generic(&json!({"a": ["x", "y", "x", "x"]}), &r);
    assert_eq!(
        codes_paths(&d),
        [("X-TST-001", "a[2]"), ("X-TST-001", "a[3]")]
    );
}

#[test]
fn unique_with_a_key_compares_that_field_of_each_table() {
    let r = one_rule(
        "",
        "path = \"a\"
check = \"unique\"
key = \"id\"",
    );
    for ok in [
        json!({"a": [{"id": "x"}, {"id": "y"}]}),
        json!({"a": [{"id": 1}, {"id": 1}]}),
        json!({"a": [{"n": "x"}, {"n": "x"}, "x", "x"]}),
    ] {
        assert!(rules::check_generic(&ok, &r).is_empty(), "{ok}");
    }
    let d = rules::check_generic(&json!({"a": [{"id": "x"}, {"id": "y"}, {"id": "x"}]}), &r);
    assert_eq!(codes_paths(&d), [("X-TST-001", "a[2].id")]);
}

/// (array path, key) for each unique target of E-UNQ-001.
const UNIQUE_TARGETS: [(&str, Option<&str>); 7] = [
    ("design.conditions", Some("id")),
    ("outcomes", Some("id")),
    ("outcomes", Some("measure")),
    ("question.hypotheses", Some("id")),
    ("inputs", Some("path")),
    ("mutations.operators", Some("name")),
    ("mutations.triggers", Some("id")),
];

fn set_path(v: &mut Value, path: &str, x: Value) {
    let (parent, last) = path.rsplit_once('.').unwrap_or(("", path));
    let mut cur = v;
    for seg in parent.split('.').filter(|s| !s.is_empty()) {
        cur = &mut cur[seg];
    }
    cur[last] = x;
}

#[test]
fn e_unq_001_fires_on_a_repeat_in_each_list_and_not_on_distinct_values() {
    let rules = v0();
    let unq = |s: &Value| -> Vec<String> {
        rules::check_generic(s, &rules)
            .into_iter()
            .filter(|d| d.code == "E-UNQ-001")
            .map(|d| d.path)
            .collect()
    };
    assert!(unq(&template()).is_empty());
    for (path, key) in UNIQUE_TARGETS {
        let item = |v: &str| match key {
            None => json!(v),
            Some(k) => json!({ k: v }),
        };
        let mut s = template();
        set_path(&mut s, path, json!([item("a"), item("b")]));
        assert!(unq(&s).is_empty(), "{path}");
        set_path(&mut s, path, json!([item("a"), item("b"), item("a")]));
        let want = match key {
            None => format!("{path}[2]"),
            Some(k) => format!("{path}[2].{k}"),
        };
        assert_eq!(unq(&s), [want], "{path}");
    }
}

#[test]
fn input_units_strata_and_the_pick_rule_are_checked_at_their_edges() {
    let rules = v0();
    let at = |ptr: &str, v: Value| -> Vec<(String, String)> {
        let mut s = template();
        apply(&mut s, ptr, Some(v));
        rules::check_generic(&s, &rules)
            .into_iter()
            .filter(|d| !d.code.starts_with("W-"))
            .map(|d| (d.code, d.path))
            .collect()
    };
    let one = |code: &str, path: &str| vec![(code.to_string(), path.to_string())];
    let units = "inputs[1].units";
    assert_eq!(at("/inputs/1/units", json!(0)), one("E-RNG-001", units));
    assert!(at("/inputs/1/units", json!(1)).is_empty());
    assert!(at("/inputs/1/units", json!(4294967295u32)).is_empty());
    assert_eq!(
        at("/inputs/1/units", json!(4294967296u64)),
        one("E-RNG-001", units)
    );
    assert_eq!(at("/inputs/1/units", json!("3")), one("E-TYP-001", units));
    let stratum = "inputs[1].stratum";
    assert_eq!(
        at("/inputs/1/stratum", json!("Fiction")),
        one("E-FMT-001", stratum)
    );
    assert!(at("/inputs/1/stratum", json!("short-fiction")).is_empty());
    assert!(at("/inputs/1/stratum", json!("")).is_empty());
    assert_eq!(at("/inputs/1/stratum", json!(3)), one("E-TYP-001", stratum));
    assert_eq!(
        at("/design/stratum", json!("by genre")),
        one("E-FMT-001", "design.stratum")
    );
    assert!(at("/design/stratum", json!("")).is_empty());
    assert_eq!(
        at("/design/pick", json!("random")),
        one("E-ENM-001", "design.pick")
    );
    assert!(at("/design/pick", json!("first")).is_empty());
    assert_eq!(
        at("/design/pick", json!(1)),
        one("E-TYP-001", "design.pick")
    );
}

/// The errors of the template with its first hypothesis's `field` set to `v`.
fn hypothesis_with(field: &str, v: Value) -> Vec<(String, String)> {
    let mut s = template();
    s["question"]["hypotheses"][0][field] = v;
    rules::check_generic(&s, &v0())
        .into_iter()
        .filter(|d| !d.code.starts_with("W-"))
        .map(|d| (d.code, d.path))
        .collect()
}

fn at_hypothesis(code: &str, path: &str) -> Vec<(String, String)> {
    vec![(code.to_string(), format!("question.hypotheses[0].{path}"))]
}

#[test]
fn hypothesis_fields_are_typed_and_ids_and_strata_have_their_form() {
    let (h, one) = (hypothesis_with, at_hypothesis);
    for id in ["E1", "P1a", "h1-b2"] {
        assert!(h("id", json!(id)).is_empty(), "{id}");
    }
    for id in ["E1.pass", "E 1", "", "E--1", "-E1", "\u{c9}1"] {
        assert_eq!(h("id", json!(id)), one("E-FMT-001", "id"), "{id}");
    }
    assert!(h("strata", json!(["code", "short-fiction"])).is_empty());
    for s in ["Code", "", "a b"] {
        assert_eq!(
            h("strata", json!(["code", s])),
            one("E-FMT-001", "strata[1]"),
            "{s}"
        );
    }
    assert_eq!(h("strata", json!([3])), one("E-TYP-001", "strata[0]"));
    assert_eq!(h("strata", json!("code")), one("E-TYP-001", "strata"));
    assert_eq!(h("outcome", json!(1)), one("E-TYP-001", "outcome"));
    assert!(h("contrast", json!({ "arm-a": 1, "arm-b": -0.5 })).is_empty());
    assert_eq!(
        h("contrast", json!({ "arm-a": "x" })),
        one("E-TYP-001", "contrast.arm-a")
    );
    assert_eq!(h("contrast", json!([1])), one("E-TYP-001", "contrast"));
    for (field, v) in [
        ("claim", json!({ "below": 1 })),
        ("claim", json!({ "above": -1.5 })),
        ("claim", json!({ "within": [-1, 1] })),
        ("statistic", json!({ "mean": [-5, 5] })),
        ("statistic", json!({ "quantile": 0.5 })),
        ("power_at", json!(1)),
        ("alpha_share", json!(0.5)),
        ("estimates", json!({ "from": "my-pilot" })),
    ] {
        assert!(h(field, v.clone()).is_empty(), "{field} = {v}");
    }
    assert_eq!(h("claim", json!(1)), one("E-TYP-001", "claim"));
    assert_eq!(
        h("claim", json!({ "below": "x" })),
        one("E-TYP-001", "claim.below")
    );
    assert_eq!(
        h("claim", json!({ "within": [1, "x"] })),
        one("E-TYP-001", "claim.within[1]")
    );
    assert_eq!(
        h("claim", json!({ "beloww": 1 })),
        one("E-KEY-001", "claim.beloww")
    );
    assert_eq!(
        h("statistic", json!({ "mean": 5 })),
        one("E-TYP-001", "statistic.mean")
    );
    assert_eq!(
        h("statistic", json!({ "quantile": "half" })),
        one("E-TYP-001", "statistic.quantile")
    );
    assert_eq!(h("power_at", json!("x")), one("E-TYP-001", "power_at"));
    assert_eq!(
        h("alpha_share", json!(true)),
        one("E-TYP-001", "alpha_share")
    );
    assert_eq!(
        h("estimates", json!("my-pilot")),
        one("E-TYP-001", "estimates")
    );
    assert_eq!(
        h("estimates", json!({ "from": 3 })),
        one("E-TYP-001", "estimates.from")
    );
    assert_eq!(
        h("estimates", json!({ "from": "My-Pilot" })),
        one("E-FMT-001", "estimates.from")
    );
    assert_eq!(
        h("estimates", json!({})),
        one("E-REQ-001", "estimates.from")
    );
}

#[test]
fn a_planted_record_hash_is_not_a_study_id() {
    let planted = format!("record:{}", "0".repeat(64));
    assert_eq!(
        hypothesis_with("estimates", json!({ "from": planted })),
        at_hypothesis("E-FMT-001", "estimates.from")
    );
}
