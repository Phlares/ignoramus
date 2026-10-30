mod common;

use ignorarium::diag::Severity;
use ignorarium::estimate::{Estimate, apply_defaults, estimate};
use ignorarium::machine::{self, Machine};
use ignorarium::rules::{self, CheckCtx, Rule};
use ignorarium::schedule::{self, Row};
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

fn input(path: &str, role: &str, units: u32) -> Value {
    json!({
        "path": path, "sha256": "", "role": role, "stratum": "fiction", "units": units,
        "licence": { "spdx": "", "type": "unspecified", "source": "", "creators": [] }
    })
}

/// `n` inputs of `role`, each holding `units` units.
fn clusters(role: &str, n: usize, units: u32) -> Vec<Value> {
    (0..n)
        .map(|i| input(&format!("data/{role}-{i:02}.txt"), role, units))
        .collect()
}

fn spec_of(v: &Value) -> Spec {
    Spec::from_value(v).unwrap()
}

/// 3 conditions in each of 2 stages, 4 clusters, 5 units per cluster, repeats 2, 36 s per row.
fn sized() -> Value {
    let mut v = template();
    v["design"]["conditions"] = json!([{"id": "a"}, {"id": "b"}, {"id": "c"}]);
    v["design"]["reference"] = json!("a");
    v["design"]["selection"] =
        json!({ "metric": "ppl-ratio", "conditions": ["b", "c"], "rule": "best-mean" });
    v["design"]["confirmation"] = json!({ "conditions": ["b", "c"] });
    let mut inputs = clusters("selection", 1, 9);
    inputs.extend(clusters("confirmation", 3, 5));
    v["inputs"] = json!(inputs);
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

const H: [u8; 32] = [7; 32];

fn rows_of(spec: &Spec) -> Vec<Row> {
    schedule::rows(spec, &H).unwrap()
}

#[test]
fn estimate_matches_the_brief() {
    let spec = spec_of(&sized());
    let e = estimate(&spec, &machine(4, 0.5), &rows_of(&spec));
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
fn the_estimate_counts_the_schedule_rows() {
    let spec = spec_of(&sized());
    let rows = rows_of(&spec);
    // Rows it is given, not units it works out: a part of the schedule counts as that part.
    let part: Vec<Row> = rows
        .iter()
        .filter(|r| r.condition != "b")
        .take(7)
        .cloned()
        .collect();
    let e = estimate(&spec, &machine(1, 0.0), &part);
    assert_eq!(e.rows, 7);
    let a = part.iter().filter(|r| r.condition == "a").count() as u64;
    assert_eq!(
        e.rows_per_condition,
        vec![("a".into(), a), ("b".into(), 0), ("c".into(), 7 - a)]
    );
    close(e.cpu_hours, 7.0 * 36.0 / 3600.0);
    let none = estimate(&spec, &machine(1, 0.0), &[]);
    assert_eq!(none.rows, 0);
    close(none.cpu_hours, 0.0);
}

#[test]
fn zero_cpus_count_as_one() {
    let spec = spec_of(&sized());
    let e = estimate(&spec, &machine(0, 1.0), &rows_of(&spec));
    close(e.wall_hours, 1.2);
    close(e.money_usd, 1.2);
}

#[test]
fn long_job_needs_more_than_the_limit() {
    // One condition, one row: wall minutes = row_seconds / 60.
    let mut v = template();
    v["design"]["conditions"] = json!([{"id": "a"}]);
    v["design"]["reference"] = json!("a");
    v["design"]["confirmation"] = json!({ "conditions": [] });
    v["design"]["repeats"] = json!(1);
    let m = Machine::default();
    let long = |v: &Value, m: &Machine| {
        let spec = spec_of(v);
        let rows = rows_of(&spec);
        assert_eq!(rows.len(), 1);
        estimate(&spec, m, &rows).is_long_job
    };
    v["design"]["row_seconds"] = json!(1800.0);
    assert!(!long(&v, &m));
    v["design"]["row_seconds"] = json!(1801.0);
    assert!(long(&v, &m));
    let strict = Machine {
        long_job_minutes: 29,
        ..Machine::default()
    };
    v["design"]["row_seconds"] = json!(1800.0);
    assert!(long(&v, &strict));
}

#[test]
fn label_earnable_follows_mode() {
    let mut v = template();
    let e: Estimate = estimate(&spec_of(&v), &Machine::default(), &[]);
    assert_eq!(e.label_earnable, "signal");
    v["study"]["mode"] = json!("confirmatory");
    let e = estimate(&spec_of(&v), &Machine::default(), &[]);
    assert_eq!(e.label_earnable, "confirmed");
}

fn defaults() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("design.alpha", "/design/alpha", json!(0.005)),
        ("design.power", "/design/power", json!(0.8)),
        ("design.repeats", "/design/repeats", json!(1)),
        ("design.pick", "/design/pick", json!("seeded")),
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
    v["design"]["pick"] = json!("first");
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
            "design.pick",
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
            "/design/pick",
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

/// A box in which `my-pilot` has finished with the template contrast's estimate.
fn ctx(dir: &Path) -> CheckCtx {
    let pilot = common::finished(vec![common::arm_a()]);
    CheckCtx {
        records: Box::new(common::Studies(vec![(
            "my-pilot",
            Ok(common::records_for("my-pilot", &pilot)),
        )])),
        ..CheckCtx::empty(dir)
    }
}

/// Confirmatory with a mean claim sized by `my-pilot`, alpha and power absent: 2 clusters
/// against the 14 needed.
fn thin() -> Value {
    let mut v = template();
    v["study"]["mode"] = json!("confirmatory");
    let h = &mut v["question"]["hypotheses"][0];
    h["claim"] = json!({ "below": 0.0 });
    h["statistic"] = json!({ "mean": [-50.0, 50.0] });
    h["power_at"] = json!(-1.0);
    h["estimates"] = json!({ "from": "my-pilot" });
    h["alpha_share"] = json!(1.0);
    v["design"]["units_per_cluster"] = json!(5);
    v["inputs"] = json!(clusters("confirmation", 2, 5));
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
    assert!(!cnf[0].problem.contains("Cannot compute power"));
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
            .all(|d| d.problem.contains("Cannot compute power"))
    );
}

/// Paths of the numeric fields in the template.
const NUMERIC: [&[&str]; 9] = [
    &["scope", "min_isolation"],
    &["scope", "budget", "cpu_hours"],
    &["scope", "budget", "wall_hours"],
    &["scope", "budget", "money_usd"],
    &["design", "repeats"],
    &["design", "row_seconds"],
    &["design", "alpha"],
    &["design", "power"],
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
    let more: [&[&str]; 2] = [&["mutations", "depth_cap"], &["mutations", "triggers"]];
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
