use ignorarium::diag::Severity;
use ignorarium::estimate::{Estimate, apply_defaults, estimate, units};
use ignorarium::machine::{self, Machine};
use ignorarium::prior::NoPriorUse;
use ignorarium::rules::{self, CheckCtx, Rule};
use ignorarium::spec::model::Spec;
use serde_json::{Value, json};
use std::path::Path;

fn template() -> Value {
    ignorarium::spec::load(Path::new("../../templates/numeric-ablation.toml"))
        .unwrap()
        .value
}

fn remove(v: &mut Value, ptr: &str) {
    let (parent, key) = ptr.rsplit_once('/').unwrap();
    v.pointer_mut(parent)
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove(key)
        .unwrap();
}

fn ids(prefix: &str, n: usize) -> Value {
    Value::Array((0..n).map(|i| json!(format!("{prefix}{i:02}"))).collect())
}

fn spec_of(v: &Value) -> Spec {
    Spec::from_value(v).unwrap()
}

/// 3 conditions, 4 clusters, 5 units per cluster, repeats 2, 36 s per row.
fn sized() -> Value {
    let mut v = template();
    v["design"]["conditions"] = json!([{"id": "a"}, {"id": "b"}, {"id": "c"}]);
    v["design"]["selection"]["clusters"] = ids("s", 1);
    v["design"]["confirmation"]["clusters"] = ids("c", 3);
    v["design"]["units_per_cluster"] = json!(5);
    v["design"]["repeats"] = json!(2);
    v["design"]["row_seconds"] = json!(36.0);
    v
}

fn machine(cpus: u32, rate: f64) -> Machine {
    Machine {
        cpus,
        rate_usd_per_hour: rate,
        ..Machine::default()
    }
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-12, "{a} vs {b}");
}

#[test]
fn estimate_matches_the_brief() {
    let spec = spec_of(&sized());
    let n = units(&spec);
    assert_eq!(n, 20);
    let e = estimate(&spec, &machine(4, 0.5), n);
    assert_eq!(e.rows, 120);
    close(e.cpu_hours, 1.2);
    close(e.wall_hours, 0.3);
    close(e.money_usd, 0.15);
    assert!(!e.is_long_job);
    assert_eq!(
        e.rows_per_condition,
        vec![("a".into(), 40), ("b".into(), 40), ("c".into(), 40)]
    );
}

#[test]
fn zero_cpus_count_as_one() {
    let e = estimate(&spec_of(&sized()), &machine(0, 1.0), 20);
    close(e.wall_hours, 1.2);
    close(e.money_usd, 1.2);
}

#[test]
fn long_job_needs_more_than_the_limit() {
    // One condition, one row: wall minutes = row_seconds / 60.
    let mut v = template();
    v["design"]["conditions"] = json!([{"id": "a"}]);
    v["design"]["repeats"] = json!(1);
    let m = Machine::default();
    v["design"]["row_seconds"] = json!(1800.0);
    assert!(!estimate(&spec_of(&v), &m, 1).is_long_job);
    v["design"]["row_seconds"] = json!(1801.0);
    assert!(estimate(&spec_of(&v), &m, 1).is_long_job);
    let strict = Machine {
        long_job_minutes: 29,
        ..Machine::default()
    };
    v["design"]["row_seconds"] = json!(1800.0);
    assert!(estimate(&spec_of(&v), &strict, 1).is_long_job);
}

#[test]
fn label_earnable_follows_mode() {
    let mut v = template();
    let e: Estimate = estimate(&spec_of(&v), &Machine::default(), 1);
    assert_eq!(e.label_earnable, "signal");
    v["study"]["mode"] = json!("confirmatory");
    let e = estimate(&spec_of(&v), &Machine::default(), 1);
    assert_eq!(e.label_earnable, "confirmed");
}

#[test]
fn units_count_distinct_clusters_and_default_to_one() {
    let mut v = template();
    v["design"]["units_per_cluster"] = json!(7);
    assert_eq!(units(&spec_of(&v)), 7);
    v["design"]["selection"]["clusters"] = json!(["x", "y"]);
    v["design"]["confirmation"]["clusters"] = json!(["y", "z"]);
    assert_eq!(units(&spec_of(&v)), 21);
}

#[test]
fn estimate_saturates() {
    let mut v = template();
    v["design"]["repeats"] = json!(u32::MAX);
    let e = estimate(&spec_of(&v), &Machine::default(), u64::MAX);
    assert_eq!(e.rows, u64::MAX);
}

fn defaults() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("design.alpha", "/design/alpha", json!(0.005)),
        ("design.power", "/design/power", json!(0.8)),
        ("design.repeats", "/design/repeats", json!(1)),
        ("mutations.depth_cap", "/mutations/depth_cap", json!(2)),
        ("scope.min_isolation", "/scope/min_isolation", json!(1)),
    ]
}

#[test]
fn each_default_is_filled_when_absent() {
    for (path, ptr, want) in defaults() {
        let mut v = template();
        remove(&mut v, ptr);
        let filled = apply_defaults(&mut v);
        assert_eq!(filled, vec![(path.to_string(), want.clone())], "{path}");
        assert_eq!(v.pointer(ptr), Some(&want));
    }
}

#[test]
fn present_values_are_left_alone() {
    let mut v = template();
    v["design"]["alpha"] = json!(0.01);
    v["design"]["power"] = json!(0.9);
    v["design"]["repeats"] = json!(3);
    v["mutations"]["depth_cap"] = json!(1);
    v["scope"]["min_isolation"] = json!(2);
    v["access"] = json!({ "blind": false });
    let before = v.clone();
    assert!(apply_defaults(&mut v).is_empty());
    assert_eq!(v, before);
}

#[test]
fn blind_follows_mode() {
    for (mode, want) in [("confirmatory", true), ("exploratory", false)] {
        let mut v = template();
        v["study"]["mode"] = json!(mode);
        v.as_object_mut().unwrap().remove("access");
        let filled = apply_defaults(&mut v);
        assert_eq!(filled, vec![("access.blind".to_string(), json!(want))]);
        assert_eq!(v["access"]["blind"], json!(want));
    }
}

fn paths(filled: Vec<(String, Value)>) -> Vec<String> {
    filled.into_iter().map(|(p, _)| p).collect()
}

#[test]
fn absent_parent_is_created_and_non_table_parent_skipped() {
    let mut v = json!({});
    assert_eq!(
        paths(apply_defaults(&mut v)),
        [
            "design.alpha",
            "design.power",
            "design.repeats",
            "mutations.depth_cap",
            "scope.min_isolation",
            "access.blind"
        ]
    );
    let mut v = json!({ "design": 5, "scope": [] });
    assert_eq!(
        paths(apply_defaults(&mut v)),
        ["mutations.depth_cap", "access.blind"]
    );
    assert_eq!(v["design"], json!(5));
    assert_eq!(v["scope"], json!([]));
    let mut v = json!(7);
    assert!(apply_defaults(&mut v).is_empty());
    assert_eq!(v, json!(7));
}

#[test]
fn machine_absent_gives_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let m = machine::load(dir.path()).unwrap();
    assert_eq!(
        m,
        Machine {
            name: "local".into(),
            cpus: 1,
            mem_gib: 1,
            max_level: 0,
            rate_usd_per_hour: 0.0,
            experiment_mode: false,
            long_job_minutes: 30,
        }
    );
}

#[test]
fn machine_partial_file_keeps_other_defaults() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("machine.toml"),
        "name = \"box\"\ncpus = 8\nrate_usd_per_hour = 0.25\n",
    )
    .unwrap();
    let m = machine::load(dir.path()).unwrap();
    assert_eq!(m.name, "box");
    assert_eq!(m.cpus, 8);
    assert_eq!(m.rate_usd_per_hour, 0.25);
    assert_eq!(m.long_job_minutes, 30);
    assert_eq!(m.mem_gib, 1);
}

#[test]
fn machine_bad_file_is_an_error_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("machine.toml");
    for text in ["cpus = [", "cpus = \"many\"\n"] {
        std::fs::write(&file, text).unwrap();
        let err = machine::load(dir.path()).unwrap_err().to_string();
        assert!(err.contains("machine.toml"), "{err}");
        assert!(err.contains("line"), "{err}");
        assert!(err.contains("column"), "{err}");
    }
}

#[test]
fn machine_file_that_cannot_be_read_is_an_error_not_the_defaults() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("machine.toml")).unwrap();
    let err = machine::load(dir.path()).unwrap_err().to_string();
    assert!(err.contains("machine.toml"), "{err}");
}

#[test]
fn machine_out_of_range_values_name_the_field() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("machine.toml");
    let cases = [
        ("rate_usd_per_hour = -0.01", "rate_usd_per_hour"),
        ("rate_usd_per_hour = inf", "rate_usd_per_hour"),
        ("rate_usd_per_hour = nan", "rate_usd_per_hour"),
        ("cpus = 0", "cpus"),
        ("long_job_minutes = 0", "long_job_minutes"),
    ];
    for (text, field) in cases {
        std::fs::write(&file, text).unwrap();
        let err = machine::load(dir.path()).unwrap_err().to_string();
        assert!(
            err.contains("machine.toml") && err.contains(field),
            "{text}: {err}"
        );
    }
    for ok in ["rate_usd_per_hour = 0", "cpus = 1", "long_job_minutes = 1"] {
        std::fs::write(&file, ok).unwrap();
        machine::load(dir.path()).unwrap();
    }
}

#[test]
fn blind_default_is_false_without_a_usable_mode() {
    for mut v in [json!({}), json!({ "study": { "mode": 5 } })] {
        let filled = apply_defaults(&mut v);
        assert!(filled.contains(&("access.blind".to_string(), json!(false))));
        assert_eq!(v["access"]["blind"], json!(false));
    }
}

#[test]
fn defaults_table_matches_the_templates() {
    for t in ["numeric-ablation", "planted-spawn"] {
        let full = ignorarium::spec::load(Path::new(&format!("../../templates/{t}.toml")))
            .unwrap()
            .value;
        for ptr in [
            "/design/alpha",
            "/design/power",
            "/design/repeats",
            "/mutations/depth_cap",
            "/scope/min_isolation",
            "/access/blind",
        ] {
            let mut v = full.clone();
            remove(&mut v, ptr);
            let filled = apply_defaults(&mut v);
            assert_eq!(filled.len(), 1, "{t} {ptr}");
            assert_eq!(v, full, "{t} {ptr}");
        }
    }
}

fn v0() -> Vec<Rule> {
    rules::load(Path::new("../../rules/v0.toml")).unwrap()
}

fn ctx(dir: &Path) -> CheckCtx {
    CheckCtx {
        study_dir: dir.to_path_buf(),
        prior: Box::new(NoPriorUse),
        machine_max_level: 2,
        spec_text: String::new(),
    }
}

/// Confirmatory, alpha and power absent, 10 confirmation units against 54 needed.
fn thin() -> Value {
    let mut v = template();
    v["study"]["mode"] = json!("confirmatory");
    v["design"]["sesoi"] = json!(1.0);
    v["design"]["variance"]["value"] = json!(4.0);
    v["design"]["units_per_cluster"] = json!(5);
    v["design"]["confirmation"]["clusters"] = ids("c", 2);
    remove(&mut v, "/design/alpha");
    remove(&mut v, "/design/power");
    v
}

#[test]
fn check_all_fills_defaults_before_the_power_gate() {
    let dir = tempfile::tempdir().unwrap();
    let v = thin();
    let (out, filled, diags) = rules::check_all(&v, &v0(), &ctx(dir.path()));
    assert_eq!(out["design"]["alpha"], json!(0.005));
    assert_eq!(out["design"]["power"], json!(0.8));
    assert_eq!(paths(filled), ["design.alpha", "design.power"]);
    let cnf: Vec<_> = diags.iter().filter(|d| d.code == "E-CNF-002").collect();
    assert_eq!(cnf.len(), 1);
    assert!(!cnf[0].problem.contains("cannot compute power"));
    // The input is untouched.
    assert!(v["design"].get("alpha").is_none());
}

#[test]
fn check_named_alone_fails_closed_without_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let d = rules::check_named(&thin(), &v0(), &ctx(dir.path()));
    let cnf: Vec<_> = d.iter().filter(|d| d.code == "E-CNF-002").collect();
    assert!(!cnf.is_empty());
    assert!(
        cnf.iter()
            .all(|d| d.problem.contains("cannot compute power"))
    );
}

/// Paths of the numeric fields in the template.
const NUMERIC: [&[&str]; 11] = [
    &["scope", "min_isolation"],
    &["scope", "budget", "cpu_hours"],
    &["scope", "budget", "wall_hours"],
    &["scope", "budget", "money_usd"],
    &["design", "repeats"],
    &["design", "row_seconds"],
    &["design", "alpha"],
    &["design", "power"],
    &["design", "sesoi"],
    &["design", "icc"],
    &["design", "units_per_cluster"],
];

#[test]
fn a_spec_that_passes_check_always_deserializes() {
    let rules = v0();
    let dir = tempfile::tempdir().unwrap();
    let mut specs = vec![template()];
    let sweep = [
        json!(-1),
        json!(0),
        json!(1),
        json!(2),
        json!(4294967296u64),
        json!(1.5),
        json!(i64::MAX),
    ];
    let more: [&[&str]; 3] = [
        &["design", "variance", "value"],
        &["mutations", "depth_cap"],
        &["mutations", "triggers"],
    ];
    for path in NUMERIC.iter().chain(more.iter()) {
        for x in &sweep {
            let mut v = template();
            let mut cur = &mut v;
            for k in &path[..path.len() - 1] {
                cur = &mut cur[*k];
            }
            let last = path[path.len() - 1];
            cur[last] = if last == "triggers" {
                json!([{ "id": "T1", "metric": "m", "threshold_e": x }])
            } else {
                x.clone()
            };
            specs.push(v);
        }
    }
    let mut passing = 0;
    for v in specs {
        let (out, _, diags) = rules::check_all(&v, &rules, &ctx(dir.path()));
        if !diags.iter().any(|d| d.severity == Severity::Error) {
            passing += 1;
            Spec::from_value(&out).unwrap_or_else(|e| panic!("{v}: {e}"));
        }
    }
    assert!(passing > 10);
}
