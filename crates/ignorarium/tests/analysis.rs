use ignorarium::analysis::{paired, select};
use ignorarium::derived::{Candidate, Selection};
use ignorarium::records::Record;
use ignorarium::spec::model::Spec;
use serde_json::json;

fn spec(kind: &str) -> Spec {
    let mut v = ignorarium::spec::load("../../templates/numeric-ablation.toml".as_ref())
        .unwrap()
        .value;
    v["outcomes"][0]["measure"] = json!("ppl");
    v["outcomes"][0]["paired"] = json!(kind);
    v["design"]["reference"] = json!("base");
    Spec::from_value(&v).unwrap()
}

fn row(unit: &str, cond: &str, repeat: u32, exit: &str, ppl: Option<f64>) -> Record {
    let mut body = json!({ "condition": cond, "unit": unit, "repeat": repeat, "exit": exit });
    if let Some(p) = ppl {
        body["values"] = json!({ "ppl": p });
    }
    Record {
        seq: 0,
        prev: String::new(),
        study: "s".into(),
        row: Some(0),
        kind: "row".into(),
        body,
        ts: String::new(),
    }
}

fn ok(unit: &str, cond: &str, repeat: u32, ppl: f64) -> Record {
    row(unit, cond, repeat, "ok", Some(ppl))
}

#[test]
fn percent_pairs_by_unit_and_repeat() {
    let rows = [
        ok("a#0", "base", 0, 100.0),
        ok("a#0", "new", 0, 110.0),
        ok("a#1", "base", 0, 50.0),
        ok("a#1", "new", 0, 40.0),
    ];
    let p = paired(&spec("percent"), &rows);
    let got = p.get("a#0", "new", "ppl-ratio").unwrap();
    assert!((got - 10.0).abs() < 1e-9, "{got}");
    let got = p.get("a#1", "new", "ppl-ratio").unwrap();
    assert!((got + 20.0).abs() < 1e-9, "{got}");
    // The reference against itself pairs to zero; an unknown outcome has no value.
    assert_eq!(p.get("a#0", "base", "ppl-ratio"), Some(0.0));
    assert_eq!(p.get("a#0", "new", "other"), None);
    assert_eq!(p.units().collect::<Vec<_>>(), ["a#0", "a#1"]);
}

#[test]
fn difference_is_condition_minus_reference_and_repeats_average() {
    let rows = [
        ok("a#0", "base", 0, 100.0),
        ok("a#0", "new", 0, 90.0),
        ok("a#0", "base", 1, 100.0),
        ok("a#0", "new", 1, 70.0),
        // Repeat 2 has no reference row: it does not pair.
        ok("a#0", "new", 2, 0.0),
    ];
    let p = paired(&spec("difference"), &rows);
    assert_eq!(p.get("a#0", "new", "ppl-ratio"), Some(-20.0));
}

#[test]
fn a_unit_without_its_reference_row_has_no_paired_value() {
    let rows = [
        ok("a#0", "new", 0, 110.0),
        ok("a#1", "base", 0, 100.0),
        // A void reference row and a payload failure do not count.
        ok("a#2", "new", 0, 110.0),
        row("a#2", "base", 0, "void:output", None),
        ok("a#3", "base", 0, 100.0),
        row("a#3", "new", 0, "payload:1", None),
        // A record of another kind never counts.
        Record {
            kind: "spend".into(),
            ..ok("a#4", "new", 0, 1.0)
        },
        ok("a#4", "base", 0, 1.0),
    ];
    let p = paired(&spec("percent"), &rows);
    for (u, c) in [
        ("a#0", "new"),
        ("a#2", "new"),
        ("a#3", "new"),
        ("a#4", "new"),
    ] {
        assert_eq!(p.get(u, c, "ppl-ratio"), None, "{u}");
    }
}

#[test]
fn a_zero_reference_has_no_percent_value() {
    let rows = [
        ok("a#0", "base", 0, 0.0),
        ok("a#0", "new", 0, 5.0),
        ok("a#1", "base", 0, -1.0),
        ok("a#1", "new", 0, 5.0),
        ok("a#2", "base", 0, 1e-9),
        ok("a#2", "new", 0, 2e-9),
    ];
    let p = paired(&spec("percent"), &rows);
    assert_eq!(p.get("a#0", "new", "ppl-ratio"), None);
    assert_eq!(p.get("a#1", "new", "ppl-ratio"), None);
    let tiny = p.get("a#2", "new", "ppl-ratio").unwrap();
    assert!((tiny - 100.0).abs() < 1e-6, "{tiny}");
    // The difference form has no such limit.
    let d = paired(&spec("difference"), &rows);
    assert_eq!(d.get("a#0", "new", "ppl-ratio"), Some(5.0));
    assert_eq!(d.get("a#1", "new", "ppl-ratio"), Some(6.0));
}

#[test]
fn a_pair_that_overflows_is_absent() {
    let rows = [
        ok("a#0", "base", 0, 1e-308),
        ok("a#0", "new", 0, 1e308),
        ok("a#1", "base", 0, -1e308),
        ok("a#1", "new", 0, 1e308),
    ];
    assert_eq!(
        paired(&spec("percent"), &rows).get("a#0", "new", "ppl-ratio"),
        None
    );
    let d = paired(&spec("difference"), &rows);
    assert_eq!(d.get("a#1", "new", "ppl-ratio"), None);
    // The mean of finite pairs that sum past the range is absent too.
    let rows = [
        ok("a#2", "base", 0, -0.5e308),
        ok("a#2", "new", 0, 1e308),
        ok("a#2", "base", 1, -0.5e308),
        ok("a#2", "new", 1, 1e308),
    ];
    let d = paired(&spec("difference"), &rows);
    assert_eq!(d.get("a#2", "new", "ppl-ratio"), None);
}

/// A selection stage over `candidates`, against the reference `base`, ranking the paired
/// difference of the template's one outcome in `direction`.
fn staged(candidates: &[&str], direction: &str) -> Spec {
    let mut v = ignorarium::spec::load("../../templates/numeric-ablation.toml".as_ref())
        .unwrap()
        .value;
    v["outcomes"][0]["paired"] = json!("difference");
    v["outcomes"][0]["direction"] = json!(direction);
    v["design"]["reference"] = json!("base");
    v["design"]["selection"] =
        json!({ "metric": "ppl-ratio", "conditions": candidates, "rule": "best-mean" });
    v["design"]["confirmation"] = json!({ "conditions": ["selected"] });
    Spec::from_value(&v).unwrap()
}

/// `r` as a row of `stage`.
fn in_stage(stage: &str, mut r: Record) -> Record {
    r.body["stage"] = json!(stage);
    r
}

/// An `ok` selection-stage row.
fn sel(unit: &str, cond: &str, ppl: f64) -> Record {
    in_stage("selection", ok(unit, cond, 0, ppl))
}

/// A selection-stage row that recorded no values.
fn sel_failed(unit: &str, cond: &str, exit: &str) -> Record {
    in_stage("selection", row(unit, cond, 0, exit, None))
}

fn candidates(s: &Selection) -> Vec<(&str, f64)> {
    s.candidates
        .iter()
        .map(|c| (c.id.as_str(), c.mean))
        .collect()
}

#[test]
fn select_picks_the_best_mean_by_the_outcome_direction() {
    let rows = [
        sel("s#0", "base", 100.0),
        sel("s#0", "a", 110.0),
        sel("s#0", "b", 105.0),
        sel("s#1", "base", 100.0),
        sel("s#1", "a", 90.0),
        sel("s#1", "b", 101.0),
    ];
    let lower = select(&staged(&["a", "b"], "lower"), &rows).unwrap();
    assert_eq!(
        lower,
        Selection {
            metric: "ppl-ratio".into(),
            candidates: vec![
                Candidate {
                    id: "a".into(),
                    mean: 0.0
                },
                Candidate {
                    id: "b".into(),
                    mean: 3.0
                },
            ],
            selected: "a".into(),
            complete_units: 2,
            incomplete_units: 0,
        }
    );
    let higher = select(&staged(&["a", "b"], "higher"), &rows).unwrap();
    assert_eq!(higher.selected, "b");
    assert_eq!(candidates(&higher), [("a", 0.0), ("b", 3.0)]);
}

#[test]
fn a_tie_goes_to_the_first_listed() {
    let rows = [
        sel("s#0", "base", 10.0),
        sel("s#0", "a", 12.0),
        sel("s#0", "b", 11.0),
        sel("s#1", "base", 10.0),
        sel("s#1", "a", 12.0),
        sel("s#1", "b", 13.0),
    ];
    for direction in ["lower", "higher"] {
        for (order, first) in [(["a", "b"], "a"), (["b", "a"], "b")] {
            let s = select(&staged(&order, direction), &rows).unwrap();
            assert_eq!(s.selected, first, "{direction} {order:?}");
            assert_eq!(s.candidates[0].mean, s.candidates[1].mean);
        }
    }
}

#[test]
fn incomplete_units_stay_out_of_the_pick() {
    let rows = [
        // No value for b: out of the pick, though a is best here.
        sel("s#0", "base", 100.0),
        sel("s#0", "a", 0.0),
        sel_failed("s#0", "b", "void:output"),
        // A void reference row pairs nothing.
        sel_failed("s#1", "base", "void:output"),
        sel("s#1", "a", 1.0),
        sel("s#1", "b", 1.0),
        sel("s#2", "base", 100.0),
        sel("s#2", "a", 105.0),
        sel("s#2", "b", 101.0),
        // Confirmation rows are not selection units, whatever their condition.
        in_stage("confirmation", ok("c#0", "base", 0, 100.0)),
        in_stage("confirmation", ok("c#0", "a", 0, 0.0)),
    ];
    let s = select(&staged(&["a", "b"], "lower"), &rows).unwrap();
    assert_eq!(candidates(&s), [("a", 5.0), ("b", 1.0)]);
    assert_eq!(s.selected, "b");
    assert_eq!((s.complete_units, s.incomplete_units), (1, 2));
}

#[test]
fn select_refuses_without_a_complete_unit_a_stage_or_a_finite_mean() {
    let spec = staged(&["a", "b"], "lower");
    let rows = [
        sel("s#0", "base", 100.0),
        sel("s#0", "a", 90.0),
        sel_failed("s#0", "b", "payload:2"),
        sel("s#1", "base", 100.0),
        sel_failed("s#1", "a", "void:output"),
    ];
    assert_eq!(
        select(&spec, &rows).unwrap_err(),
        "no selection unit has a ppl-ratio value for every candidate; incomplete units: 2"
    );
    assert_eq!(
        select(&spec, &[]).unwrap_err(),
        "no selection unit has a ppl-ratio value for every candidate; incomplete units: 0"
    );
    let mut plain = spec.clone();
    plain.design.selection = None;
    assert_eq!(
        select(&plain, &rows).unwrap_err(),
        "the plan has no selection stage"
    );
    let mut other = spec.clone();
    other.design.selection.as_mut().unwrap().metric = "nope".into();
    assert_eq!(
        select(&other, &rows).unwrap_err(),
        "the selection metric is not an outcome of the plan"
    );
    let mut none = spec.clone();
    none.design.selection.as_mut().unwrap().conditions.clear();
    assert_eq!(
        select(&none, &rows).unwrap_err(),
        "the selection stage lists no candidate"
    );
    // Two finite pairs whose sum overflows.
    let big = [
        sel("s#0", "base", 0.0),
        sel("s#0", "a", 1e308),
        sel("s#0", "b", 0.0),
        sel("s#1", "base", 0.0),
        sel("s#1", "a", 1e308),
        sel("s#1", "b", 0.0),
    ];
    assert_eq!(
        select(&spec, &big).unwrap_err(),
        "the mean ppl-ratio of candidate a is not finite"
    );
}

#[test]
fn a_repeat_with_a_zero_reference_drops_out_of_the_percent_mean() {
    let rows = [
        ok("a#0", "base", 0, 0.0),
        ok("a#0", "new", 0, 5.0),
        ok("a#0", "base", 1, 100.0),
        ok("a#0", "new", 1, 120.0),
    ];
    let p = paired(&spec("percent"), &rows);
    let got = p.get("a#0", "new", "ppl-ratio").unwrap();
    assert!((got - 20.0).abs() < 1e-9, "{got}");
}
