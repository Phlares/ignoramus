use ignorarium::derived::Estimate;
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
