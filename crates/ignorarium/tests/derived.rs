use ignorarium::derived::{Estimate, Register, Selection};
use ignorarium::records::Record;
use serde_json::{Value, json};

fn rec(kind: &str, body: Value) -> Record {
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

fn body() -> Value {
    json!({
        "hypothesis": "H1", "outcome": "ppl-ratio", "key": {}, "key_hash": "k", "stratum": "",
        "clusters": 4, "units": 20, "mean": 1.5, "var": 4.0, "icc": 0.25
    })
}

#[test]
fn a_valid_estimate_parses() {
    let e = Estimate::of(&rec("estimate", body())).unwrap();
    assert_eq!(
        (e.outcome.as_str(), e.var, e.icc),
        ("ppl-ratio", 4.0, Some(0.25))
    );
    assert_eq!((e.clusters, e.units, e.mean), (4, 20, 1.5));
}

#[test]
fn another_kind_is_not_an_estimate() {
    assert_eq!(Estimate::of(&rec("row", body())), None);
}

#[test]
fn an_extra_field_is_not_an_estimate() {
    let mut b = body();
    b["extra"] = json!(1);
    assert_eq!(Estimate::of(&rec("estimate", b)), None);
}

fn register_body() -> Value {
    json!({
        "spec_hash": "sha256:ab", "leaf": 3, "log": "planlog", "defaults": [],
        "root": "/work/study", "payload": "p".repeat(64),
        "inputs": [["data/a.txt", "d".repeat(64)]]
    })
}

#[test]
fn a_register_record_parses_with_its_root() {
    let r = Register::of(&rec("register", register_body())).unwrap();
    assert_eq!(
        (
            r.spec_hash.as_str(),
            r.leaf,
            r.log.as_str(),
            r.root.as_str()
        ),
        ("sha256:ab", 3, "planlog", "/work/study")
    );
    assert_eq!(r.payload, "p".repeat(64));
    assert_eq!(r.inputs, [("data/a.txt".to_string(), "d".repeat(64))]);
    assert_eq!(r.timestamp, None);
    let mut b = register_body();
    b["timestamp"] = json!("pending");
    let r = Register::of(&rec("register", b)).unwrap();
    assert_eq!(r.timestamp.as_deref(), Some("pending"));
}

#[test]
fn a_register_record_with_a_missing_or_extra_field_does_not_parse() {
    assert_eq!(Register::of(&rec("estimate", register_body())), None);
    for field in [
        "spec_hash",
        "leaf",
        "log",
        "defaults",
        "root",
        "payload",
        "inputs",
    ] {
        let mut b = register_body();
        b.as_object_mut().unwrap().remove(field);
        assert_eq!(Register::of(&rec("register", b)), None, "{field}");
    }
    let mut b = register_body();
    b["extra"] = json!(1);
    assert_eq!(Register::of(&rec("register", b)), None);
}

fn selection_body() -> Value {
    json!({
        "metric": "ppl-ratio", "rule": "best-mean", "direction": "lower",
        "candidates": [{ "id": "a", "mean": 1.5 }, { "id": "b", "mean": -2.0 }],
        "selected": "b", "units": ["s#0", "s#3"], "complete_units": 2, "incomplete_units": 1
    })
}

#[test]
fn a_selection_record_parses_and_only_as_its_kind() {
    let s = Selection::of(&rec("selection", selection_body())).unwrap();
    assert_eq!(s.selected, "b");
    assert_eq!((s.complete_units, s.incomplete_units), (2, 1));
    assert_eq!(s.units, ["s#0", "s#3"]);
    assert_eq!(s.candidates[1].mean, -2.0);
    assert_eq!(serde_json::to_value(&s).unwrap(), selection_body());
    assert_eq!(Selection::of(&rec("estimate", selection_body())), None);
    let mut b = selection_body();
    b["candidates"][0]["extra"] = json!(1);
    assert_eq!(Selection::of(&rec("selection", b)), None);
}
