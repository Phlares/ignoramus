use ignorarium::canon::{sha256_hex, to_canonical};
use ignorarium::derived::{Analysis, Estimate, Register, Selection, StudyState, contrast_key};
use ignorarium::records::Record;
use ignorarium::spec::model::{Direction, Outcome, Paired};
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
        "inputs": [["data/a.txt", "d".repeat(64)]],
        "resolved": [{
            "path": "question.hypotheses[0].estimates", "study": "pilot",
            "records": ["e".repeat(64)], "value": { "": { "sd": 2.0, "icc": 0.1 } }
        }]
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
    assert_eq!(r.resolved.len(), 1);
    let frozen = &r.resolved[0];
    assert_eq!(
        (frozen.path.as_str(), frozen.study.as_str()),
        ("question.hypotheses[0].estimates", "pilot")
    );
    assert_eq!(frozen.records, ["e".repeat(64)]);
    assert_eq!(frozen.value, json!({ "": { "sd": 2.0, "icc": 0.1 } }));
    assert_eq!(serde_json::to_value(&r).unwrap(), register_body());
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
        "resolved",
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

type Terms = Vec<(String, f64, Value)>;

fn ppl_ratio() -> Outcome {
    Outcome {
        id: "ppl-ratio".into(),
        unit: "ratio".into(),
        direction: Direction::Lower,
        measure: "ppl".into(),
        paired: Paired::Percent,
        registered: true,
    }
}

#[test]
fn a_contrast_key_is_canonical_json_sorted_by_condition() {
    let terms = [
        ("b".to_string(), -1.0, json!({})),
        ("a".to_string(), 1.0, json!({ "bits": 4 })),
    ];
    let reference = ("ref".to_string(), json!({}));
    let (key, hash) = contrast_key(&ppl_ratio(), &reference, &terms);
    let text = r#"{"contrast":[["a",1.0,{"bits":4}],["b",-1.0,{}]],"measure":"ppl","outcome":"ppl-ratio","paired":"percent","reference":["ref",{}]}"#;
    assert_eq!(
        String::from_utf8(to_canonical(&key).unwrap()).unwrap(),
        text
    );
    assert_eq!(hash, sha256_hex(text.as_bytes()));
    assert_eq!(
        hash,
        "99f75d6845c62b078f2e76f3e5937bf61cedc3f1b6a19860ccd1e1374c0c0810"
    );
    // The order the terms come in does not matter; each part of the key does.
    let reversed = [terms[1].clone(), terms[0].clone()];
    assert_eq!(contrast_key(&ppl_ratio(), &reference, &reversed).1, hash);
    let other = |edit: &dyn Fn(&mut Outcome, &mut Terms)| {
        let (mut o, mut t) = (ppl_ratio(), terms.to_vec());
        edit(&mut o, &mut t);
        contrast_key(&o, &reference, &t).1
    };
    let changed = [
        other(&|o, _| o.id = "ppl".into()),
        other(&|o, _| o.measure = "nll".into()),
        other(&|o, _| o.paired = Paired::Difference),
        other(&|_, t| t[0].1 = -2.0),
        other(&|_, t| t[1].2 = json!({ "bits": 8 })),
        other(&|_, t| t[0].0 = "c".into()),
        other(&|_, t| {
            t.pop();
        }),
    ];
    for h in &changed {
        assert_ne!(*h, hash);
    }
    // A paired value is taken against the reference: another reference, or its other
    // params, is another contrast.
    for other in [("exact", json!({})), ("ref", json!({ "bits": 16 }))] {
        let reference = (other.0.to_string(), other.1);
        assert_ne!(contrast_key(&ppl_ratio(), &reference, &terms).1, hash);
    }
    // Unit, direction and registration do not change what the values are.
    assert_eq!(other(&|o, _| o.unit = "percent".into()), hash);
    assert_eq!(other(&|o, _| o.direction = Direction::Higher), hash);
    assert_eq!(other(&|o, _| o.registered = false), hash);
}

#[test]
fn a_contrast_key_orders_two_weights_of_one_condition() {
    let a = |w: f64| ("a".to_string(), w, json!({}));
    let reference = ("ref".to_string(), json!({}));
    let (key, hash) = contrast_key(&ppl_ratio(), &reference, &[a(1.0), a(-1.0)]);
    assert_eq!(key["contrast"], json!([["a", -1.0, {}], ["a", 1.0, {}]]));
    assert_eq!(
        contrast_key(&ppl_ratio(), &reference, &[a(-1.0), a(1.0)]).1,
        hash
    );
}

fn analysis_body() -> Value {
    json!({
        "commit": "c".repeat(64), "box_version": "0.1.0", "estimates": 2, "verdicts": 1,
        "slot": 3
    })
}

#[test]
fn an_analysis_record_states_what_follows_it() {
    let a = Analysis::of(&rec("analysis", analysis_body())).unwrap();
    assert_eq!(
        a,
        Analysis {
            commit: "c".repeat(64),
            box_version: "0.1.0".into(),
            estimates: 2,
            verdicts: 1,
            slot: Some(3),
        }
    );
    assert_eq!(serde_json::to_value(&a).unwrap(), analysis_body());
    let mut b = analysis_body();
    b["slot"] = Value::Null;
    assert_eq!(
        Analysis::of(&rec("analysis", b)).unwrap().slot,
        None,
        "an exploratory study holds no slot"
    );
    assert_eq!(Analysis::of(&rec("estimate", analysis_body())), None);
    for field in ["commit", "box_version", "estimates", "verdicts", "slot"] {
        let mut b = analysis_body();
        b.as_object_mut().unwrap().remove(field);
        assert_eq!(Analysis::of(&rec("analysis", b)), None, "{field}");
    }
}

/// A record whose hash is its own: `seq` tells two records with one body apart.
fn at(seq: u64, kind: &str, body: Value) -> Record {
    Record {
        seq,
        ..rec(kind, body)
    }
}

fn estimate_for(hypothesis: &str, stratum: &str) -> Value {
    let mut b = body();
    b["hypothesis"] = json!(hypothesis);
    b["stratum"] = json!(stratum);
    b
}

#[test]
fn a_study_is_finished_once_every_record_its_analysis_promises_follows_it() {
    assert_eq!(StudyState::of(None), StudyState::NotRegistered);
    let register = at(0, "register", register_body());
    let analysis = at(2, "analysis", analysis_body());
    let e1 = at(3, "estimate", estimate_for("H1", ""));
    let e2 = at(4, "estimate", estimate_for("H1", "code"));
    let verdict = at(5, "verdict", json!({ "hypothesis": "H1" }));
    let state =
        |records: &[&Record]| StudyState::of(Some(records.iter().map(|r| (*r).clone()).collect()));
    // No analysis record yet, or one whose estimates and verdict are not all there. A crash
    // after the analysis record leaves the study unfinished, not finished without values.
    assert_eq!(state(&[&register]), StudyState::NotFinished);
    assert_eq!(state(&[&register, &analysis]), StudyState::NotFinished);
    assert_eq!(
        state(&[&register, &analysis, &e1, &e2]),
        StudyState::NotFinished
    );
    assert_eq!(
        state(&[&register, &analysis, &e1, &verdict]),
        StudyState::NotFinished
    );
    // A repeat of an estimate is the same estimate; one written before the analysis does
    // not count.
    let again = at(6, "estimate", estimate_for("H1", ""));
    assert_eq!(
        state(&[&register, &analysis, &e1, &again, &verdict]),
        StudyState::NotFinished
    );
    assert_eq!(
        state(&[&register, &e2, &analysis, &e1, &verdict]),
        StudyState::NotFinished
    );
    // Another value for the same hypothesis, key and stratum is no second estimate.
    let mut other = estimate_for("H1", "");
    other["mean"] = json!(2.5);
    let other = at(7, "estimate", other);
    assert_eq!(
        state(&[&register, &analysis, &e1, &other, &verdict]),
        StudyState::NotFinished
    );
    // A finished study gives a reader only what follows its analysis record: an estimate
    // written before it is not one the analysis promised.
    let all = [&register, &e2, &analysis, &e1, &e2, &verdict];
    assert_eq!(
        state(&all),
        StudyState::Finished(vec![e1.clone(), e2.clone(), verdict.clone()])
    );
    // An analysis record that does not parse promises nothing a reader can trust.
    let torn = at(2, "analysis", json!({ "commit": "c" }));
    assert_eq!(state(&[&register, &torn]), StudyState::NotFinished);
    // An analysis that promises nothing finishes the study at once.
    let mut nothing = analysis_body();
    nothing["estimates"] = json!(0);
    nothing["verdicts"] = json!(0);
    let bare = [register.clone(), at(2, "analysis", nothing)];
    assert_eq!(
        StudyState::of(Some(bare.to_vec())),
        StudyState::Finished(vec![])
    );
}
