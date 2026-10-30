use ignorarium::canon::sha256_hex;
use ignorarium::diag::{Diagnostic, Severity};
use ignorarium::prior::{NoPriorUse, PriorError, PriorUse, normalized_sha256};
use ignorarium::records::Record;
use ignorarium::rules::{self, BoxRecords, CheckCtx, Rule};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::Path;
use std::rc::Rc;

fn v0() -> &'static [Rule] {
    static RULES: std::sync::OnceLock<Vec<Rule>> = std::sync::OnceLock::new();
    RULES.get_or_init(|| rules::load("../../rules/v0.toml".as_ref()).unwrap())
}

fn spec_at(path: &str) -> Value {
    ignorarium::spec::load(Path::new(path)).unwrap().value
}

fn ctx(dir: &Path) -> CheckCtx {
    ctx_with(dir, Box::new(NoPriorUse))
}

fn ctx_with(dir: &Path, prior: Box<dyn PriorUse>) -> CheckCtx {
    CheckCtx {
        prior,
        records: Box::new(Lookup(Ok(Some(record(
            "estimate",
            estimate("ppl-ratio", 4.0),
        ))))),
        ..CheckCtx::empty(dir)
    }
}

/// A box that answers every lookup the same way.
struct Lookup(Result<Option<Record>, String>);

impl BoxRecords for Lookup {
    fn find(&self, _: &str) -> Result<Option<Record>, String> {
        self.0.clone()
    }
}

fn record(kind: &str, body: Value) -> Record {
    Record {
        seq: 0,
        prev: "0".repeat(64),
        study: "donor".into(),
        row: None,
        kind: kind.into(),
        body,
        ts: "2026-01-01T00:00:00Z".into(),
    }
}

fn estimate(outcome: &str, var: f64) -> Value {
    json!({
        "hypothesis": "H1", "outcome": outcome, "key": {}, "key_hash": "k", "stratum": "",
        "clusters": 4, "units": 20, "mean": 1.0, "var": var, "icc": null
    })
}

/// A box whose one estimate holds the variance the plan states, so only the plan's other
/// faults show.
fn holding(spec: &Value) -> Box<dyn BoxRecords> {
    let var = spec["design"]["variance"]["value"].as_f64().unwrap_or(4.0);
    Box::new(Lookup(Ok(Some(record(
        "estimate",
        estimate("ppl-ratio", var),
    )))))
}

fn named(spec: &Value, dir: &Path) -> Vec<Diagnostic> {
    let c = CheckCtx {
        records: holding(spec),
        ..ctx(dir)
    };
    rules::check_named(spec, v0(), &c)
}

fn codes_paths(d: &[Diagnostic]) -> Vec<(&str, &str)> {
    d.iter()
        .map(|x| (x.code.as_str(), x.path.as_str()))
        .collect()
}

fn set(spec: &mut Value, ptr: &str, v: Option<Value>) {
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

fn ids(prefix: &str, n: usize) -> Value {
    Value::Array((0..n).map(|i| json!(format!("{prefix}{i:02}"))).collect())
}

/// A confirmatory plan that passes every named check: 55 confirmation units against 54 needed.
fn ok() -> Value {
    let mut s = spec_at("../../templates/numeric-ablation.toml");
    let edits = [
        ("/study/mode", json!("confirmatory")),
        ("/design/sesoi", json!(1.0)),
        ("/design/variance/value", json!(4.0)),
        (
            "/design/variance/source",
            json!(format!("record:{}", "a".repeat(64))),
        ),
        ("/design/units_per_cluster", json!(5)),
        ("/design/selection/clusters", ids("s", 3)),
        ("/design/confirmation/clusters", ids("c", 11)),
        (
            "/mutations/triggers",
            json!([{ "id": "T1", "metric": "ppl-ratio", "threshold_e": 1.0 }]),
        ),
    ];
    for (p, v) in edits {
        set(&mut s, p, Some(v));
    }
    s
}

type Edit = (&'static str, Option<Value>);

/// Edits on `ok()`, then the exact diagnostics (code, path) in rule-file order.
struct Fault {
    edits: Vec<Edit>,
    want: Vec<(&'static str, &'static str)>,
}

fn f(edits: Vec<Edit>, want: Vec<(&'static str, &'static str)>) -> Fault {
    Fault { edits, want }
}

fn j(v: Value) -> Option<Value> {
    Some(v)
}

/// Edits that turn on a follow-on route with these entries.
fn route(follow_on: Value) -> Vec<Edit> {
    vec![
        ("/scope/end_of_run/route", j(json!("follow-on"))),
        ("/scope/end_of_run/follow_on", j(follow_on)),
    ]
}

fn one(when: &str, spec: &str) -> Value {
    json!([{ "when": when, "spec": spec }])
}

fn power_edits(upc: u32, clusters: usize) -> Vec<Edit> {
    vec![
        ("/design/units_per_cluster", j(json!(upc))),
        ("/design/confirmation/clusters", j(ids("c", clusters))),
    ]
}

/// Two clusters of 5 units, and one edit: small enough that the gate fires if it computes.
fn guard(ptr: &'static str, v: Value) -> Vec<Edit> {
    let mut e = power_edits(5, 2);
    e.push((ptr, j(v)));
    e
}

fn faults() -> Vec<Fault> {
    // Built at run time: the leak gate rejects a drive prefix written in a source file.
    let drive: &'static str = Box::leak(format!("{}:{}", 'C', "/Windows/x.txt").into_boxed_str());
    let pth = |p: &'static str| {
        f(
            vec![("/inputs/0/path", j(json!(p)))],
            vec![("E-PTH-001", "inputs[0].path")],
        )
    };
    vec![
        f(
            vec![
                ("/study/mode", j(json!("exploratory"))),
                ("/study/label", j(json!("replicated"))),
            ],
            vec![("E-LBL-001", "study.label")],
        ),
        // Zero and negative: neither also fires the power gate.
        f(
            vec![("/design/sesoi", j(json!(0.0)))],
            vec![("E-CNF-001", "design.sesoi")],
        ),
        f(
            vec![("/design/sesoi", j(json!(-1.0)))],
            vec![("E-CNF-001", "design.sesoi")],
        ),
        // 53 units against 54 needed; 51 against 54 with clusters of 3.
        f(
            power_edits(1, 53),
            vec![("E-CNF-002", "design.confirmation.clusters")],
        ),
        f(
            power_edits(3, 17),
            vec![("E-CNF-002", "design.confirmation.clusters")],
        ),
        // A repeated id counts once: 11 copies of one cluster are 5 units, a doubled list of 30 is 30.
        f(
            vec![
                ("/design/units_per_cluster", j(json!(5))),
                ("/design/confirmation/clusters", j(json!(vec!["c00"; 11]))),
            ],
            vec![("E-CNF-002", "design.confirmation.clusters")],
        ),
        f(
            vec![
                ("/design/units_per_cluster", j(json!(1))),
                (
                    "/design/confirmation/clusters",
                    j(json!(
                        ids("c", 30)
                            .as_array()
                            .unwrap()
                            .iter()
                            .chain(ids("c", 30).as_array().unwrap())
                            .collect::<Vec<_>>()
                    )),
                ),
            ],
            vec![("E-CNF-002", "design.confirmation.clusters")],
        ),
        f(
            vec![("/design/variance/value", j(json!(0.0)))],
            vec![("E-CNF-002", "design.confirmation.clusters")],
        ),
        f(
            vec![("/design/variance/value", j(json!(-4.0)))],
            vec![("E-CNF-002", "design.confirmation.clusters")],
        ),
        f(
            vec![("/design/variance/source", j(json!("stated")))],
            vec![("E-CNF-003", "design.variance.source")],
        ),
        // Overlap at the second element; an empty confirmation also fails the power gate.
        f(
            vec![("/design/selection/clusters", j(json!(["s00", "c01"])))],
            vec![("E-CNF-004", "design.confirmation.clusters[1]")],
        ),
        f(
            vec![("/design/confirmation/clusters", j(json!([])))],
            vec![
                ("E-CNF-002", "design.confirmation.clusters"),
                ("E-CNF-004", "design.confirmation.clusters"),
            ],
        ),
        f(
            vec![("/design/reference", j(json!("exakt")))],
            vec![("E-REF-001", "design.reference")],
        ),
        // The reference may be any condition; an empty condition list is reported by non-empty alone.
        f(vec![("/design/reference", j(json!("arm-a")))], vec![]),
        f(vec![("/design/conditions", j(json!([])))], vec![]),
        f(
            vec![("/design/conditions", j(json!([{ "id": "exakt" }, 5])))],
            vec![("E-REF-001", "design.reference")],
        ),
        f(
            vec![("/design/test", j(json!("descriptive")))],
            vec![("E-CNF-006", "design.test")],
        ),
        f(
            vec![
                ("/scope/budget/cpu_hours", j(json!(0.0))),
                ("/scope/budget/wall_hours", j(json!(0))),
                ("/scope/budget/money_usd", j(json!(0.0))),
            ],
            vec![("E-BUD-001", "scope.budget")],
        ),
        f(
            vec![
                ("/scope/budget/cpu_hours", None),
                ("/scope/budget/wall_hours", None),
                ("/scope/budget/money_usd", None),
            ],
            vec![("E-BUD-001", "scope.budget")],
        ),
        f(
            route(json!([])),
            vec![("E-END-001", "scope.end_of_run.follow_on")],
        ),
        f(
            route(one("H9.pass", "f.toml")),
            vec![("E-END-001", "scope.end_of_run.follow_on[0].when")],
        ),
        f(
            route(one("H1.maybe", "f.toml")),
            vec![("E-END-001", "scope.end_of_run.follow_on[0].when")],
        ),
        f(
            route(one("H1", "f.toml")),
            vec![("E-END-001", "scope.end_of_run.follow_on[0].when")],
        ),
        f(
            route(one(".pass", "f.toml")),
            vec![("E-END-001", "scope.end_of_run.follow_on[0].when")],
        ),
        f(
            route(one("H1.fail", "missing.toml")),
            vec![("E-END-002", "scope.end_of_run.follow_on[0].spec")],
        ),
        f(
            vec![("/mutations/depth_cap", j(json!(3)))],
            vec![("E-MUT-001", "mutations.depth_cap")],
        ),
        f(
            vec![("/mutations/seeds/report", j(json!("spawn/")))],
            vec![("E-MUT-002", "mutations.seeds")],
        ),
        f(
            vec![("/mutations/seeds/spawn", j(json!("report/x")))],
            vec![("E-MUT-002", "mutations.seeds")],
        ),
        f(
            vec![("/mutations/seeds/report", j(json!("spawn/x")))],
            vec![("E-MUT-002", "mutations.seeds")],
        ),
        f(
            vec![("/mutations/triggers/0/metric", j(json!("nope")))],
            vec![("E-MUT-003", "mutations.triggers[0].metric")],
        ),
        // machine_max_level is 2.
        f(
            vec![("/scope/min_isolation", j(json!(3)))],
            vec![("E-ISO-001", "scope.min_isolation")],
        ),
        pth("../secret.txt"),
        pth("data/../../x.txt"),
        pth("/etc/passwd"),
        pth(drive),
        pth("E:data.txt"),
        pth("data\\x.txt"),
        pth(""),
        f(
            route(one("H1.pass", "../next.toml")),
            vec![("E-PTH-001", "scope.end_of_run.follow_on[0].spec")],
        ),
        // An unsafe path is never read: no hash or freshness check follows it.
        f(
            vec![
                ("/inputs/0/path", j(json!("../secret.txt"))),
                ("/inputs/0/sha256", j(json!("b".repeat(64)))),
                ("/inputs/0/role", j(json!("confirmation"))),
            ],
            vec![("E-PTH-001", "inputs[0].path")],
        ),
        // Overflow: the product of user integers saturates; the plan then passes.
        f(
            vec![("/design/units_per_cluster", j(json!(i64::MAX)))],
            vec![],
        ),
        f(
            vec![("/design/units_per_cluster", j(json!(-5)))],
            vec![("E-CNF-002", "design.confirmation.clusters")],
        ),
        // Power-gate guards: with 2 clusters the gate fires if it computes, so [] means it skipped.
        f(guard("/design/alpha", json!(0.0)), vec![]),
        f(guard("/design/alpha", json!(1.0)), vec![]),
        f(guard("/design/power", json!(0.0)), vec![]),
        f(guard("/design/power", json!(1.0)), vec![]),
        f(guard("/design/icc", json!(1.0)), vec![]),
        f(guard("/design/icc", json!(-0.1)), vec![]),
        f(guard("/design/alpha", json!("x")), vec![]),
        f(
            guard("/design/alpha", json!(0.45)),
            vec![("E-CNF-002", "design.confirmation.clusters")],
        ),
        f(
            guard("/design/power", json!(0.999)),
            vec![("E-CNF-002", "design.confirmation.clusters")],
        ),
        f(
            guard("/design/power", json!(0.5)),
            vec![("E-CNF-002", "design.confirmation.clusters")],
        ),
        f(
            guard("/design/icc", json!(0.0)),
            vec![("E-CNF-002", "design.confirmation.clusters")],
        ),
        // Fail closed: an absent alpha, power or icc cannot be computed.
        f(
            vec![("/design/alpha", None)],
            vec![("E-CNF-002", "design.confirmation.clusters")],
        ),
        f(
            vec![("/design/power", None)],
            vec![("E-CNF-002", "design.confirmation.clusters")],
        ),
        f(
            vec![("/design/icc", None)],
            vec![("E-CNF-002", "design.confirmation.clusters")],
        ),
        // An empty array as an element is not an empty array-level field, and is no cluster.
        f(
            vec![(
                "/design/confirmation/clusters",
                j(json!(
                    ids("c", 11)
                        .as_array()
                        .unwrap()
                        .iter()
                        .chain(&[json!([])])
                        .collect::<Vec<_>>()
                )),
            )],
            vec![],
        ),
        f(route(json!([{ "when": [], "spec": "f.toml" }])), vec![]),
        // A budget of negatives is not a budget.
        f(
            vec![
                ("/scope/budget/cpu_hours", j(json!(-1.0))),
                ("/scope/budget/wall_hours", j(json!(0.0))),
                ("/scope/budget/money_usd", j(json!(0.0))),
            ],
            vec![("E-BUD-001", "scope.budget")],
        ),
        // Path forms: a lone `..`, a colon, and a segment Windows would trim.
        pth(".."),
        pth("a/b:c"),
        pth("a. /x"),
        pth("a./x"),
        pth("a/."),
        f(
            route(one("H1.pass", "..")),
            vec![("E-PTH-001", "scope.end_of_run.follow_on[0].spec")],
        ),
    ]
}

/// The study folder every table row runs in: `f.toml` exists, `data/example.txt` does not.
fn study_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.toml"), "x = 1\n").unwrap();
    dir
}

fn edited(edits: &[Edit]) -> Value {
    let mut s = ok();
    for (p, v) in edits {
        set(&mut s, p, v.clone());
    }
    s
}

#[test]
fn checks_read_files_from_the_context_root() {
    let dir = study_dir();
    let other = tempfile::tempdir().unwrap();
    assert_eq!(CheckCtx::empty(dir.path()).root, dir.path());
    let spec = edited(&route(one("H1.fail", "f.toml")));
    assert!(
        !codes_paths(&named(&spec, dir.path()))
            .contains(&("E-END-002", "scope.end_of_run.follow_on[0].spec"))
    );
    assert!(
        codes_paths(&named(&spec, other.path()))
            .contains(&("E-END-002", "scope.end_of_run.follow_on[0].spec"))
    );
}

#[test]
fn the_base_plan_passes_every_named_check() {
    let dir = study_dir();
    assert!(named(&ok(), dir.path()).is_empty());
}

#[test]
fn a_variance_record_must_exist_and_hold_the_stated_value() {
    let dir = study_dir();
    let cases = [
        (Ok(None), "No record in this box has this hash."),
        (
            Ok(Some(record("row", estimate("ppl-ratio", 4.0)))),
            "The record is not an estimate.",
        ),
        (
            Ok(Some(record("estimate", estimate("other", 4.0)))),
            "The record is for an outcome this plan does not list.",
        ),
        (
            Ok(Some(record("estimate", estimate("ppl-ratio", 5.0)))),
            "The record holds variance 5, and the plan states 4.",
        ),
        (
            Err("line 2 is not a whole record".to_string()),
            "The box cannot read its records: line 2 is not a whole record",
        ),
    ];
    for (found, detail) in cases {
        let c = CheckCtx {
            records: Box::new(Lookup(found)),
            ..CheckCtx::empty(dir.path())
        };
        let d = rules::check_named(&ok(), v0(), &c);
        assert_eq!(codes_paths(&d), [("E-CNF-003", "design.variance.source")]);
        let want = "The variance record is missing, is not an estimate for an outcome of this plan, or holds another value.";
        assert_eq!(d[0].problem, format!("{want} {detail}"));
    }
    let same = CheckCtx {
        records: Box::new(Lookup(Ok(Some(record(
            "estimate",
            estimate("ppl-ratio", 4.0),
        ))))),
        ..CheckCtx::empty(dir.path())
    };
    assert!(rules::check_named(&ok(), v0(), &same).is_empty());
}

#[test]
fn each_fault_row_yields_exactly_its_codes_and_paths() {
    let dir = study_dir();
    for row in faults() {
        let d = named(&edited(&row.edits), dir.path());
        assert_eq!(codes_paths(&d), row.want, "{:?}", row.edits);
    }
}

#[test]
fn the_named_rules_are_the_expected_set_and_rows_cover_the_pure_ones() {
    let in_file: BTreeSet<_> = v0()
        .iter()
        .filter(|r| matches!(r.check, rules::Check::Named(_)))
        .map(|r| r.code.clone())
        .collect();
    let want: BTreeSet<String> = [
        "E-LBL-001",
        "E-CNF-001",
        "E-CNF-002",
        "E-CNF-003",
        "E-CNF-004",
        "E-CNF-005",
        "E-CNF-006",
        "E-REF-001",
        "E-BUD-001",
        "E-END-001",
        "E-END-002",
        "E-MUT-001",
        "E-MUT-002",
        "E-MUT-003",
        "E-ISO-001",
        "E-PTH-001",
        "E-HSH-001",
        "E-SEC-001",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(in_file, want);
    // E-CNF-005 and E-HSH-001 read files; their tests are below. E-SEC-001 is in tests/scan.rs.
    let in_rows: BTreeSet<String> = faults()
        .iter()
        .flat_map(|r| r.want.iter().map(|w| w.0.to_string()))
        .collect();
    let missing: Vec<_> = want
        .difference(&in_rows)
        .filter(|c| !matches!(c.as_str(), "E-CNF-005" | "E-HSH-001" | "E-SEC-001"))
        .collect();
    assert!(missing.is_empty(), "{missing:?}");
}

#[test]
fn boundaries_hold_on_both_sides() {
    let dir = study_dir();
    let passes = |edits: Vec<Edit>| {
        let d = named(&edited(&edits), dir.path());
        assert!(d.is_empty(), "{edits:?}: {d:?}");
    };
    // 54 units pass where 53 fail (table).
    passes(power_edits(1, 54));
    passes(power_edits(3, 18));
    passes(vec![("/mutations/depth_cap", j(json!(2)))]);
    passes(vec![("/mutations/depth_cap", j(json!(0)))]);
    passes(vec![("/scope/min_isolation", j(json!(2)))]);
    passes(vec![("/scope/min_isolation", j(json!(0)))]);
    passes(vec![
        ("/mutations/seeds/spawn", j(json!("a/"))),
        ("/mutations/seeds/report", j(json!("b/"))),
    ]);
    for key in ["cpu_hours", "wall_hours", "money_usd"] {
        let mut s = edited(&[
            ("/scope/budget/cpu_hours", j(json!(0.0))),
            ("/scope/budget/wall_hours", j(json!(0.0))),
            ("/scope/budget/money_usd", j(json!(0.0))),
        ]);
        s["scope"]["budget"][key] = json!(0.5);
        assert!(named(&s, dir.path()).is_empty(), "{key}");
    }
    // A `..` component is unsafe; a name that contains two dots is not.
    passes(vec![("/inputs/0/path", j(json!("data/a..b.txt")))]);
    // `a//b` is the same as `a/b`.
    passes(vec![("/inputs/0/path", j(json!("data//x.txt")))]);
    for when in ["H1.pass", "H1.fail"] {
        passes(route(one(when, "f.toml")));
    }
    // The planted-spawn template: depth 2, declared trigger, exploratory.
    let p = spec_at("../../templates/planted-spawn.toml");
    assert!(named(&p, dir.path()).is_empty());
}

#[test]
fn power_gate_problem_states_both_numbers() {
    let dir = study_dir();
    let d = named(&edited(&power_edits(1, 10)), dir.path());
    assert_eq!(d.len(), 1);
    assert_eq!(
        d[0].problem,
        "The confirmation sample is too small. 10 confirmation units; the power gate needs 54."
    );
    let d = named(
        &edited(&[("/design/variance/value", j(json!(0.0)))]),
        dir.path(),
    );
    assert_eq!(
        d[0].problem,
        "The confirmation sample is too small. The variance must be > 0."
    );
}

#[test]
fn power_gate_uses_alpha_power_icc_and_defaults() {
    let dir = study_dir();
    // alpha 0.05, sd 1, sesoi 0.5 needs 32 units.
    let mut e = power_edits(1, 32);
    e.extend([
        ("/design/alpha", j(json!(0.05))),
        ("/design/variance/value", j(json!(1.0))),
        ("/design/sesoi", j(json!(0.5))),
    ]);
    assert!(named(&edited(&e), dir.path()).is_empty());
    e[1] = ("/design/confirmation/clusters", j(ids("c", 31)));
    assert_eq!(
        codes_paths(&named(&edited(&e), dir.path())),
        [("E-CNF-002", "design.confirmation.clusters")]
    );
    // Higher power needs more units.
    let mut e = power_edits(1, 54);
    e.push(("/design/power", j(json!(0.9))));
    assert_eq!(named(&edited(&e), dir.path()).len(), 1);
    // An intra-cluster correlation of 0.1 raises the need to 75; the plan has 55.
    let d = named(&edited(&[("/design/icc", j(json!(0.1)))]), dir.path());
    assert!(
        d[0].problem
            .ends_with("55 confirmation units; the power gate needs 75."),
        "{}",
        d[0].problem
    );
}

#[test]
fn confirmatory_rules_do_not_fire_in_exploratory_mode() {
    let dir = study_dir();
    let s = edited(&[
        ("/study/mode", j(json!("exploratory"))),
        ("/design/sesoi", j(json!(0.0))),
        ("/design/test", j(json!("descriptive"))),
        ("/design/variance/source", j(json!("stated"))),
        ("/design/confirmation/clusters", j(json!([]))),
    ]);
    assert!(named(&s, dir.path()).is_empty());
    // And E-LBL-001 does not fire in confirmatory mode.
    let s = edited(&[("/study/label", j(json!("confirmed")))]);
    assert!(named(&s, dir.path()).is_empty());
}

fn junk() -> Vec<Value> {
    vec![
        json!("text"),
        json!(7),
        json!(i64::MAX),
        json!(-1.5),
        json!([]),
        json!([1, "a"]),
        json!({}),
        json!(true),
    ]
}

fn nodes(v: &Value, ptr: &str, out: &mut Vec<String>) {
    let children: Vec<(String, &Value)> = match v {
        Value::Object(m) => m.iter().map(|(k, c)| (format!("{ptr}/{k}"), c)).collect(),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .map(|(i, c)| (format!("{ptr}/{i}"), c))
            .collect(),
        _ => vec![],
    };
    for (p, c) in children {
        out.push(p.clone());
        nodes(c, &p, out);
    }
}

#[test]
fn named_checks_never_panic_on_missing_or_mistyped_fields() {
    let dir = study_dir();
    assert!(named(&json!({}), dir.path()).is_empty());
    let template = spec_at("../../templates/numeric-ablation.toml");
    let strings: serde_json::Map<String, Value> = template
        .as_object()
        .unwrap()
        .keys()
        .map(|k| (k.clone(), json!("x")))
        .collect();
    named(&Value::Object(strings), dir.path());
    // A rich confirmatory plan with every node replaced by each junk value, and removed.
    let mut base = edited(&route(one("H1.pass", "f.toml")));
    set(&mut base, "/inputs/0/role", j(json!("confirmation")));
    set(&mut base, "/inputs/0/path", j(json!("f.toml")));
    let mut ptrs = vec![];
    nodes(&base, "", &mut ptrs);
    assert!(ptrs.len() > 100);
    let mut ran = 0;
    for ptr in &ptrs {
        for v in junk() {
            let mut s = base.clone();
            set(&mut s, ptr, Some(v));
            named(&s, dir.path());
            ran += 1;
        }
        let parent = ptr.rsplit_once('/').unwrap().0;
        if base.pointer(parent).is_some_and(Value::is_object) {
            let mut s = base.clone();
            set(&mut s, ptr, None);
            named(&s, dir.path());
        }
    }
    assert!(ran > 600);
}

// ---- checks that read files ----

const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn with_input(path: &str, sha: &str, role: &str) -> Value {
    let mut s = ok();
    set(&mut s, "/inputs/0/path", j(json!(path)));
    set(&mut s, "/inputs/0/sha256", j(json!(sha)));
    set(&mut s, "/inputs/0/role", j(json!(role)));
    s
}

fn write(dir: &Path, rel: &str, bytes: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

#[test]
fn input_hash_must_match_the_file() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "data/a.txt", b"abc");
    let run = |path: &str, sha: &str| named(&with_input(path, sha, "calibration"), dir.path());
    assert!(run("data/a.txt", ABC).is_empty());
    let hsh = [("E-HSH-001", "inputs[0].sha256")];
    assert_eq!(codes_paths(&run("data/a.txt", &"0".repeat(64))), hsh);
    // A missing file with a hash fires. Without a hash only the generic warning applies.
    assert_eq!(codes_paths(&run("data/none.txt", ABC)), hsh);
    assert!(run("data/none.txt", "").is_empty());
    // An empty hash on an existing file is not compared.
    assert!(run("data/a.txt", "").is_empty());
    // A folder is not a file.
    assert_eq!(codes_paths(&run("data", ABC)), hsh);
}

#[test]
fn missing_hash_is_a_warning_from_the_generic_rule() {
    let mut s = spec_at("../../templates/numeric-ablation.toml");
    let w = |s: &Value| -> Vec<Diagnostic> {
        rules::check_generic(s, v0())
            .into_iter()
            .filter(|x| x.code == "W-HSH-001")
            .collect()
    };
    let d = w(&s);
    assert_eq!(codes_paths(&d), [("W-HSH-001", "inputs[0].sha256")]);
    assert_eq!(d[0].severity, Severity::Warning);
    assert_eq!(
        d[0].instruction,
        "Pin the input: add its SHA-256, 64 lowercase hex characters."
    );
    set(&mut s, "/inputs/0/sha256", j(json!(ABC)));
    assert!(w(&s).is_empty());
}

#[test]
fn follow_on_files_are_found_under_the_study_folder() {
    let dir = tempfile::tempdir().unwrap();
    let s = edited(&route(json!([
        { "when": "H1.pass", "spec": "follow/a.toml" },
        { "when": "H1.fail", "spec": "follow/b.toml" },
    ])));
    write(dir.path(), "follow/a.toml", b"x = 1\n");
    assert_eq!(
        codes_paths(&named(&s, dir.path())),
        [("E-END-002", "scope.end_of_run.follow_on[1].spec")]
    );
    write(dir.path(), "follow/b.toml", b"x = 1\n");
    assert!(named(&s, dir.path()).is_empty());
}

type Seen = Rc<RefCell<Vec<(String, String)>>>;

struct Fake {
    exact: Option<String>,
    normalized: Option<String>,
    seen: Seen,
}

impl PriorUse for Fake {
    fn used(&self, exact: &str, normalized: &str) -> Result<Option<String>, PriorError> {
        self.seen
            .borrow_mut()
            .push((exact.into(), normalized.into()));
        if self.exact.as_deref() == Some(exact) {
            return Ok(Some("entry-exact".into()));
        }
        if self.normalized.as_deref() == Some(normalized) {
            return Ok(Some("entry-norm".into()));
        }
        Ok(None)
    }
}

struct Broken;

impl PriorUse for Broken {
    fn used(&self, _: &str, _: &str) -> Result<Option<String>, PriorError> {
        Err(PriorError("leaf 3 is not a leaf".into()))
    }
}

#[test]
fn an_unreadable_plan_log_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "d.txt", b"abc");
    let d = fresh(
        &with_input("d.txt", "", "confirmation"),
        dir.path(),
        Box::new(Broken),
    );
    assert_eq!(codes_paths(&d), [("E-CNF-005", "inputs[0].path")]);
    assert!(
        d[0].problem
            .ends_with("The plan log cannot be read: leaf 3 is not a leaf."),
        "{}",
        d[0].problem
    );
}

fn fake(exact: Option<&str>, normalized: Option<&str>) -> (Box<dyn PriorUse>, Seen) {
    let seen: Seen = Rc::default();
    let p = Fake {
        exact: exact.map(Into::into),
        normalized: normalized.map(Into::into),
        seen: seen.clone(),
    };
    (Box::new(p), seen)
}

fn fresh(spec: &Value, dir: &Path, prior: Box<dyn PriorUse>) -> Vec<Diagnostic> {
    let c = CheckCtx {
        records: holding(spec),
        ..ctx_with(dir, prior)
    };
    rules::check_named(spec, v0(), &c)
}

#[test]
fn reused_selection_or_confirmation_data_is_refused_naming_the_entry() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "d.txt", b"abc");
    for role in ["selection", "confirmation"] {
        let (prior, seen) = fake(Some(ABC), None);
        let d = fresh(&with_input("d.txt", "", role), dir.path(), prior);
        assert_eq!(codes_paths(&d), [("E-CNF-005", "inputs[0].path")], "{role}");
        assert!(
            d[0].problem.ends_with("The earlier entry is entry-exact."),
            "{}",
            d[0].problem
        );
        assert_eq!(seen.borrow().len(), 1);
    }
    // Other roles are not asked.
    for role in ["calibration", "model"] {
        let (prior, seen) = fake(Some(ABC), None);
        assert!(fresh(&with_input("d.txt", "", role), dir.path(), prior).is_empty());
        assert!(seen.borrow().is_empty(), "{role}");
    }
    // Unknown data passes.
    let (prior, _) = fake(Some("nope"), Some("nope"));
    assert!(fresh(&with_input("d.txt", "", "confirmation"), dir.path(), prior).is_empty());
}

#[test]
fn reuse_is_found_through_the_normalized_hash_too() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "d.txt", b"Alpha \r\n  BETA\n");
    let (prior, seen) = fake(None, Some(&normalized_sha256("alpha beta")));
    let d = fresh(&with_input("d.txt", "", "confirmation"), dir.path(), prior);
    assert!(
        d[0].problem.ends_with("The earlier entry is entry-norm."),
        "{}",
        d[0].problem
    );
    assert_eq!(seen.borrow()[0].0, sha256_hex(b"Alpha \r\n  BETA\n"));
}

#[test]
fn non_utf8_data_passes_an_empty_normalized_hash() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = [0xff_u8, 0xfe, 0x00];
    write(dir.path(), "d.bin", &bytes);
    let (prior, seen) = fake(None, None);
    assert!(fresh(&with_input("d.bin", "", "selection"), dir.path(), prior).is_empty());
    assert_eq!(*seen.borrow(), [(sha256_hex(&bytes), String::new())]);
}

#[test]
fn freshness_skips_missing_unsafe_and_exploratory_inputs() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "d.txt", b"abc");
    let known = normalized_sha256("abc");
    let (prior, seen) = fake(Some(ABC), Some(&known));
    assert!(
        fresh(
            &with_input("gone.txt", "", "confirmation"),
            dir.path(),
            prior
        )
        .is_empty()
    );
    assert!(seen.borrow().is_empty());
    let (prior, seen) = fake(Some(ABC), Some(&known));
    let d = fresh(
        &with_input("../d.txt", "", "confirmation"),
        dir.path(),
        prior,
    );
    assert_eq!(codes_paths(&d), [("E-PTH-001", "inputs[0].path")]);
    assert!(seen.borrow().is_empty());
    let (prior, seen) = fake(Some(ABC), None);
    let mut s = with_input("d.txt", "", "confirmation");
    set(&mut s, "/study/mode", j(json!("exploratory")));
    assert!(fresh(&s, dir.path(), prior).is_empty());
    assert!(seen.borrow().is_empty());
}

#[cfg(unix)]
#[test]
fn symlinks_under_the_study_folder_are_refused_and_never_read() {
    use std::os::unix::fs::symlink;
    let outer = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    write(outer.path(), "secret.txt", b"abc");
    write(dir.path(), "real/x.txt", b"abc");
    symlink(outer.path(), dir.path().join("linkdir")).unwrap();
    symlink(
        outer.path().join("secret.txt"),
        dir.path().join("linkfile.txt"),
    )
    .unwrap();
    symlink(dir.path().join("real"), dir.path().join("inner")).unwrap();
    for path in ["linkdir/secret.txt", "linkfile.txt", "inner/x.txt"] {
        let (prior, seen) = fake(Some(ABC), None);
        let d = fresh(&with_input(path, ABC, "confirmation"), dir.path(), prior);
        assert_eq!(codes_paths(&d), [("E-PTH-001", "inputs[0].path")], "{path}");
        assert!(seen.borrow().is_empty(), "{path}");
    }
    let s = edited(&route(one("H1.pass", "linkfile.txt")));
    assert_eq!(
        codes_paths(&named(&s, dir.path())),
        [("E-PTH-001", "scope.end_of_run.follow_on[0].spec")]
    );
    // A linked folder on the way to a follow-on spec.
    let s = edited(&route(one("H1.pass", "inner/x.txt")));
    assert_eq!(
        codes_paths(&named(&s, dir.path())),
        [("E-PTH-001", "scope.end_of_run.follow_on[0].spec")]
    );
    // A real path passes.
    assert!(named(&with_input("real/x.txt", ABC, "calibration"), dir.path()).is_empty());
}

#[test]
fn absent_power_fields_say_which_is_missing() {
    let dir = study_dir();
    for field in ["alpha", "power", "icc"] {
        let ptr: &'static str = Box::leak(format!("/design/{field}").into_boxed_str());
        let d = named(&edited(&[(ptr, None)]), dir.path());
        assert_eq!(d.len(), 1, "{field}");
        assert_eq!(
            d[0].problem,
            format!(
                "The confirmation sample is too small. Cannot compute power: design.{field} is absent."
            )
        );
    }
    // Two absent fields give two diagnostics; a mistyped one among them skips the gate.
    let d = named(
        &edited(&[("/design/alpha", None), ("/design/icc", None)]),
        dir.path(),
    );
    assert_eq!(d.len(), 2);
    assert!(
        named(
            &edited(&[("/design/alpha", None), ("/design/power", j(json!("x")))]),
            dir.path()
        )
        .is_empty()
    );
}

#[cfg(unix)]
#[test]
fn a_pipe_is_never_opened() {
    let dir = tempfile::tempdir().unwrap();
    let ok = std::process::Command::new("mkfifo")
        .arg(dir.path().join("pipe"))
        .status()
        .unwrap()
        .success();
    assert!(ok);
    // Run in a thread: a check that opens the pipe blocks forever.
    let (tx, rx) = std::sync::mpsc::channel();
    let (spec, path) = (
        with_input("pipe", ABC, "confirmation"),
        dir.path().to_path_buf(),
    );
    std::thread::spawn(move || {
        let d = named(&spec, &path);
        let got: Vec<_> = d.iter().map(|x| (x.code.clone(), x.path.clone())).collect();
        let _ = tx.send(got);
    });
    let got = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the check blocked on a pipe");
    assert_eq!(
        got,
        [("E-HSH-001".to_string(), "inputs[0].sha256".to_string())]
    );
}

// ---- rule loading and gating ----

fn rule_file(extra: &str, check: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("r.toml");
    let text = format!(
        "[[rule]]\ncode = \"X-TST-001\"\nseverity = \"warning\"\nproblem = \"p\"\ninstruction = \"i\"\nwhy = \"w\"\nsources = []\n{extra}\n\
         [[rule.target]]\npath = \"mutations.depth_cap\"\ncheck = \"{check}\"\n"
    );
    std::fs::write(&file, text).unwrap();
    (dir, file)
}

#[test]
fn an_unknown_named_check_is_a_load_error_naming_the_code() {
    let (_dir, file) = rule_file("", "named:no-such-check");
    let e = rules::load(&file).unwrap_err().to_string();
    assert!(
        e.contains("X-TST-001") && e.contains("mutations.depth_cap") && e.contains("no-such-check"),
        "{e}"
    );
}

#[test]
fn a_named_rule_gates_on_when_like_a_generic_one() {
    let (dir, file) = rule_file("when = { mode = \"confirmatory\" }", "named:depth-cap");
    let r = rules::load(&file).unwrap();
    let mut s = edited(&[("/mutations/depth_cap", j(json!(9)))]);
    let c = ctx(dir.path());
    let d = rules::check_named(&s, &r, &c);
    assert_eq!(codes_paths(&d), [("X-TST-001", "mutations.depth_cap")]);
    assert_eq!(d[0].severity, Severity::Warning);
    set(&mut s, "/study/mode", j(json!("exploratory")));
    assert!(rules::check_named(&s, &r, &c).is_empty());
}

#[test]
fn bal_brief_is_refused_with_three_codes() {
    let dir = Path::new("tests/fixtures/specs");
    let s = spec_at("tests/fixtures/specs/bal-brief.toml");
    let confirm = std::fs::read(dir.join("bal-confirm.txt")).unwrap();
    let (prior, _) = fake(Some(&sha256_hex(&confirm)), None);
    let d = fresh(&s, dir, prior);
    let codes: BTreeSet<_> = d.iter().map(|x| x.code.as_str()).collect();
    for want in ["E-CNF-002", "E-CNF-005", "E-CNF-006"] {
        assert!(codes.contains(want), "{want}: {codes:?}");
    }
    let power = d.iter().find(|x| x.code == "E-CNF-002").unwrap();
    assert!(
        power
            .problem
            .ends_with("20 confirmation units; the power gate needs 54."),
        "{}",
        power.problem
    );
}

#[test]
fn power_gate_numbers_at_the_guard_edges() {
    let dir = study_dir();
    let detail = |edits: Vec<Edit>| {
        let d = named(&edited(&edits), dir.path());
        assert_eq!(d.len(), 1, "{edits:?}");
        d[0].problem.rsplit_once(". ").unwrap().1.to_string()
    };
    // power 0.999 needs 140 units at sd 2, sesoi 1.
    let mut e = power_edits(1, 139);
    e.push(("/design/power", j(json!(0.999))));
    assert_eq!(
        detail(e),
        "139 confirmation units; the power gate needs 140."
    );
    // icc just under 1 with 5 units per cluster: the design effect is about 5.
    let mut e = power_edits(5, 11);
    e.push(("/design/icc", j(json!(0.999999))));
    assert_eq!(
        detail(e),
        "55 confirmation units; the power gate needs 267."
    );
    // No units per cluster: zero units, sized as if there were one.
    let mut e = power_edits(0, 11);
    e.push(("/design/icc", j(json!(0.1))));
    assert_eq!(detail(e), "0 confirmation units; the power gate needs 54.");
    // The largest integer saturates instead of overflowing.
    let mut e = power_edits(1, 54);
    e.push(("/design/units_per_cluster", j(json!(i64::MAX))));
    assert!(named(&edited(&e), dir.path()).is_empty());
}

/// `inputs[0].path` matches `inputs[*].path`.
fn matches_pattern(path: &str, pattern: &str) -> bool {
    let (p, q): (Vec<_>, Vec<_>) = (path.split('.').collect(), pattern.split('.').collect());
    p.len() == q.len()
        && p.iter().zip(&q).all(|(a, b)| match b.strip_suffix("[*]") {
            Some(name) => a
                .strip_prefix(name)
                .is_some_and(|r| r.starts_with('[') && r.ends_with(']')),
            None => a == b,
        })
}

#[test]
fn every_named_diagnostic_is_reported_at_a_path_its_target_matches() {
    let dir = study_dir();
    let mut seen = BTreeSet::new();
    for row in faults() {
        let d = named(&edited(&row.edits), dir.path());
        for x in &d {
            let ok = v0()
                .iter()
                .filter(|r| r.code == x.code)
                .any(|r| matches_pattern(&x.path, &r.path));
            assert!(ok, "{} at {}", x.code, x.path);
            seen.insert((x.code.clone(), x.path.clone()));
        }
    }
    assert!(seen.len() > 15, "{}", seen.len());
    assert!(matches_pattern("a[12].b", "a[*].b") && !matches_pattern("a.b", "a[*].b"));
}

// ---- inputs from the store ----

/// Put `bytes` in the store under `dir` and return its digest and meta.
fn stored(dir: &Path, bytes: &[u8]) -> (String, ignorarium::store::Meta) {
    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("f"), bytes).unwrap();
    ignorarium::store::Store::open(dir)
        .unwrap()
        .import(&src.path().join("f"))
        .unwrap()
}

#[test]
fn a_stored_input_passes_without_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let (digest, meta) = stored(dir.path(), b"Some  Text");
    let spec = with_input("data/a.txt", &digest, "calibration");
    assert!(named(&spec, dir.path()).is_empty());
    // The store comes first: a different file at the path does not matter.
    write(dir.path(), "data/a.txt", b"other bytes");
    assert!(named(&spec, dir.path()).is_empty());
    let store = ignorarium::store::Store::open(dir.path()).unwrap();
    let got = ignorarium::cli::check::input_hashes(&store, dir.path(), &spec);
    assert_eq!(
        got,
        [(
            "data/a.txt".to_string(),
            Some((digest, meta.normalized_sha256))
        )]
    );
    assert_eq!(got[0].1.as_ref().unwrap().1, normalized_sha256("some text"));
}

#[test]
fn a_stored_input_counts_for_freshness_through_its_meta() {
    let dir = tempfile::tempdir().unwrap();
    let (digest, meta) = stored(dir.path(), b"Some  Text");
    let (prior, seen) = fake(None, Some(&meta.normalized_sha256));
    let d = fresh(
        &with_input("data/a.txt", &digest, "confirmation"),
        dir.path(),
        prior,
    );
    assert_eq!(codes_paths(&d), [("E-CNF-005", "inputs[0].path")]);
    assert!(d[0].problem.ends_with("The earlier entry is entry-norm."));
    assert_eq!(*seen.borrow(), [(digest, meta.normalized_sha256)]);
}

#[test]
fn an_input_in_neither_place_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (digest, _) = stored(dir.path(), b"abc!");
    let run = |path: &str, sha: &str| named(&with_input(path, sha, "calibration"), dir.path());
    let hsh = [("E-HSH-001", "inputs[0].sha256")];
    // The pin names no stored object and no file is there.
    assert_eq!(codes_paths(&run("data/none.txt", ABC)), hsh);
    // The pin is a stored digest: it passes.
    assert!(run("data/none.txt", &digest).is_empty());
    // A pin that is no digest at all, with a file under the path whose hash differs.
    write(dir.path(), "data/a.txt", b"abc");
    assert_eq!(codes_paths(&run("data/a.txt", "xyz")), hsh);
    assert!(run("data/a.txt", ABC).is_empty());
}

#[test]
fn an_input_inside_another_inputs_path_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let with_second = |first: &str, second: &str| {
        let mut s = with_input(first, "", "calibration");
        let mut extra = s["inputs"][0].clone();
        extra["path"] = json!(second);
        s["inputs"].as_array_mut().unwrap().push(extra);
        named(&s, dir.path())
    };
    let nested = [("E-PTH-001", "inputs[1].path")];
    assert_eq!(codes_paths(&with_second("data/a", "data/a/b")), nested);
    // The inner path is flagged whichever comes first.
    assert_eq!(
        codes_paths(&with_second("data/a/b", "data/a")),
        [("E-PTH-001", "inputs[0].path")]
    );
    // A shared name prefix is not a shared folder.
    assert!(with_second("data/a", "data/ab").is_empty());
    assert!(with_second("data/a", "data/b/a").is_empty());
}

#[test]
fn a_stored_input_whose_blob_is_gone_or_changed_is_not_held() {
    let dir = tempfile::tempdir().unwrap();
    let (digest, _) = stored(dir.path(), b"abcdef");
    let blob = dir.path().join("store").join(&digest);
    let spec = with_input("data/none.txt", &digest, "calibration");
    assert!(named(&spec, dir.path()).is_empty());
    let store = ignorarium::store::Store::open(dir.path()).unwrap();
    let hsh = [("E-HSH-001", "inputs[0].sha256")];
    std::fs::remove_file(&blob).unwrap();
    assert_eq!(codes_paths(&named(&spec, dir.path())), hsh);
    // Another size: the check can afford a stat, not a read.
    std::fs::write(&blob, b"abc").unwrap();
    assert_eq!(codes_paths(&named(&spec, dir.path())), hsh);
    assert_eq!(
        ignorarium::cli::check::input_hashes(&store, dir.path(), &spec)[0].1,
        None
    );
    std::fs::remove_file(&blob).unwrap();
    std::fs::create_dir(&blob).unwrap();
    assert_eq!(codes_paths(&named(&spec, dir.path())), hsh);
}

#[test]
fn a_damaged_meta_is_reported_not_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let (digest, _) = stored(dir.path(), b"abc");
    let meta = dir.path().join("store/meta").join(format!("{digest}.json"));
    let mut perm = std::fs::metadata(&meta).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    std::fs::set_permissions(&meta, perm).unwrap();
    std::fs::write(&meta, b"not json").unwrap();
    let d = named(
        &with_input("data/none.txt", &digest, "calibration"),
        dir.path(),
    );
    assert_eq!(codes_paths(&d), [("E-HSH-001", "inputs[0].sha256")]);
    assert!(d[0].problem.contains("cannot be read"), "{}", d[0].problem);
}

#[test]
fn a_pin_on_an_unsafe_path_is_left_to_the_path_rule() {
    let dir = tempfile::tempdir().unwrap();
    let d = named(&with_input("../d.txt", ABC, "calibration"), dir.path());
    assert_eq!(codes_paths(&d), [("E-PTH-001", "inputs[0].path")]);
}

#[test]
fn two_inputs_with_one_path_are_not_nested() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = with_input("data/a", "", "calibration");
    let again = s["inputs"][0].clone();
    s["inputs"].as_array_mut().unwrap().push(again);
    assert!(named(&s, dir.path()).is_empty());
}
