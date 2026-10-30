use ignorarium::analysis::paired;
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
