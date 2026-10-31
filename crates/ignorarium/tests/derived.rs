use ignorarium::canon::{sha256_hex, to_canonical};
use ignorarium::derived::{
    Alpha, Analysis, Estimate, Instrument, Register, Selection, StudyState, contrast_key,
};
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
        "root_id": "r".repeat(64), "payload": "p".repeat(64),
        "inputs": [["data/a.txt", "d".repeat(64)]],
        "resolved": [{
            "path": "question.hypotheses[0].estimates", "study": "pilot",
            "records": ["e".repeat(64)], "value": { "": { "sd": 2.0, "icc": 0.1 } }
        }],
        "alpha": null
    })
}

#[test]
fn a_register_record_parses_with_its_root_identity() {
    let r = Register::of(&rec("register", register_body())).unwrap();
    assert_eq!(
        (
            r.spec_hash.as_str(),
            r.leaf,
            r.log.as_str(),
            r.root_id.as_str()
        ),
        ("sha256:ab", 3, "planlog", "r".repeat(64).as_str())
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
    assert_eq!(r.alpha, None);
    let mut b = register_body();
    b["timestamp"] = json!("pending");
    b["alpha"] = json!({ "log": "planlog", "slot": 4, "level": 0.01 });
    let r = Register::of(&rec("register", b.clone())).unwrap();
    assert_eq!(r.timestamp.as_deref(), Some("pending"));
    let alpha = Alpha {
        log: "planlog".into(),
        slot: 4,
        level: 0.01,
    };
    assert_eq!(r.alpha, Some(alpha));
    assert_eq!(serde_json::to_value(&r).unwrap(), b);
    b["alpha"]["extra"] = json!(1);
    assert_eq!(Register::of(&rec("register", b)), None);
}

#[test]
fn a_register_record_with_a_missing_or_extra_field_does_not_parse() {
    assert_eq!(Register::of(&rec("estimate", register_body())), None);
    for field in [
        "spec_hash",
        "leaf",
        "log",
        "defaults",
        "root_id",
        "payload",
        "inputs",
        "resolved",
        "alpha",
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
    let (key, hash) = contrast_key(&ppl_ratio(), 1, &reference, &terms);
    let text = r#"{"contrast":[["a",1.0,{"bits":4}],["b",-1.0,{}]],"measure":"ppl","outcome":"ppl-ratio","paired":"percent","reference":["ref",{}],"repeats":1}"#;
    assert_eq!(
        String::from_utf8(to_canonical(&key).unwrap()).unwrap(),
        text
    );
    assert_eq!(hash, sha256_hex(text.as_bytes()));
    assert_eq!(
        hash,
        "cc0db751ba271008a099636ef56a3e3fbe5d8d784cc64bb130cacf0c910b4fdb"
    );
    // The order the terms come in does not matter; each part of the key does.
    let reversed = [terms[1].clone(), terms[0].clone()];
    assert_eq!(contrast_key(&ppl_ratio(), 1, &reference, &reversed).1, hash);
    let other = |edit: &dyn Fn(&mut Outcome, &mut Terms)| {
        let (mut o, mut t) = (ppl_ratio(), terms.to_vec());
        edit(&mut o, &mut t);
        contrast_key(&o, 1, &reference, &t).1
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
        assert_ne!(contrast_key(&ppl_ratio(), 1, &reference, &terms).1, hash);
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
    let (key, hash) = contrast_key(&ppl_ratio(), 1, &reference, &[a(1.0), a(-1.0)]);
    assert_eq!(key["contrast"], json!([["a", -1.0, {}], ["a", 1.0, {}]]));
    assert_eq!(
        contrast_key(&ppl_ratio(), 1, &reference, &[a(-1.0), a(1.0)]).1,
        hash
    );
}

fn analysis_body() -> Value {
    json!({
        "commit": "c".repeat(64), "box_version": "0.1.0", "estimates": 2, "verdicts": 1,
        "slot": 3,
        "instrument": {
            "payload": "p".repeat(64), "environment": "host",
            "inputs": [["data/m.bin", "model", "d".repeat(64)]]
        }
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
            instrument: Instrument {
                payload: "p".repeat(64),
                environment: "host".into(),
                inputs: vec![("data/m.bin".into(), "model".into(), "d".repeat(64))],
            },
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

#[test]
fn an_analysis_record_from_before_the_instrument_names_an_empty_one() {
    // A box that wrote no instrument still leaves a finished study.
    let mut b = analysis_body();
    b.as_object_mut().unwrap().remove("instrument");
    b["estimates"] = json!(0);
    b["verdicts"] = json!(0);
    b["slot"] = Value::Null;
    let a = Analysis::of(&rec("analysis", b.clone())).unwrap();
    let empty = Instrument {
        payload: String::new(),
        environment: String::new(),
        inputs: vec![],
    };
    assert_eq!(a.instrument, empty);
    let all = vec![at(0, "register", register_body()), at(1, "analysis", b)];
    assert_eq!(
        StudyState::of(Some(all), |_, _| panic!("no slot to ask for")),
        Ok(StudyState::Finished(vec![]))
    );
    // An instrument that is there must be whole.
    for field in ["payload", "environment", "inputs"] {
        let mut b = analysis_body();
        b["instrument"].as_object_mut().unwrap().remove(field);
        assert_eq!(Analysis::of(&rec("analysis", b)), None, "{field}");
    }
    let mut b = analysis_body();
    b["instrument"]["extra"] = json!(1);
    assert_eq!(Analysis::of(&rec("analysis", b)), None);
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
    assert_eq!(
        StudyState::of(None, |_, _| panic!("no study, no slot")),
        Ok(StudyState::NotRegistered)
    );
    let register = at(0, "register", register_body());
    let analysis = at(2, "analysis", analysis_body());
    let e1 = at(3, "estimate", estimate_for("H1", ""));
    let e2 = at(4, "estimate", estimate_for("H1", "code"));
    let verdict = at(
        5,
        "verdict",
        json!({
            "hypothesis": "H1", "verdict": "none", "basis": "none", "level": null,
            "e_value": null, "bounds": [], "incomplete_units": 0, "reason": null
        }),
    );
    // Slot 3, which the analysis promises, is closed.
    let state = |records: &[&Record]| {
        let all = records.iter().map(|r| (*r).clone()).collect();
        StudyState::of(Some(all), |_, _| Ok(true)).unwrap()
    };
    // No analysis record yet, or one whose estimates and verdict are not all there. A crash
    // after the analysis record leaves the study unfinished, not finished without values.
    assert_eq!(state(&[&register]), StudyState::NotFinished);
    assert_eq!(state(&[&register, &analysis]), StudyState::NotFinished);
    assert_eq!(
        state(&[&register, &analysis, &e1, &e2]),
        StudyState::NotFinished
    );
    // A verdict record that does not parse is no verdict.
    let torn = at(8, "verdict", json!({ "hypothesis": "H1" }));
    assert_eq!(
        state(&[&register, &analysis, &e1, &e2, &torn]),
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
    // An analysis that promises nothing finishes the study at once, and asks for no slot.
    let mut nothing = analysis_body();
    nothing["estimates"] = json!(0);
    nothing["verdicts"] = json!(0);
    nothing["slot"] = Value::Null;
    let bare = [register.clone(), at(2, "analysis", nothing)];
    assert_eq!(
        StudyState::of(Some(bare.to_vec()), |_, _| panic!("no slot to ask for")),
        Ok(StudyState::Finished(vec![]))
    );
}

#[test]
fn a_study_with_a_stream_slot_is_finished_only_once_the_slot_closes() {
    let mut promised = analysis_body();
    promised["estimates"] = json!(0);
    promised["verdicts"] = json!(0);
    let all = vec![
        at(0, "register", register_body()),
        at(1, "analysis", promised),
    ];
    let asked = std::cell::RefCell::new(vec![]);
    let state = |closed: Result<bool, String>| {
        StudyState::of(Some(all.clone()), |log, slot| {
            asked.borrow_mut().push((log.to_string(), slot));
            closed
        })
    };
    // The reader asks the stream of the register record's log for the analysis's slot.
    assert_eq!(state(Ok(true)), Ok(StudyState::Finished(vec![])));
    assert_eq!(*asked.borrow(), [("planlog".to_string(), 3)]);
    assert_eq!(state(Ok(false)), Ok(StudyState::NotFinished));
    // A stream that cannot be read is an error, never "not finished".
    assert_eq!(
        state(Err("line 1 is not a whole record".into())),
        Err("line 1 is not a whole record".to_string())
    );
    // A slot with no register record to name its log is not closed.
    let no_register = vec![all[1].clone()];
    assert_eq!(
        StudyState::of(Some(no_register), |_, _| Ok(true)),
        Ok(StudyState::NotFinished)
    );
}

#[test]
fn a_contrast_key_names_the_repeats() {
    let terms = [("a".to_string(), 1.0, json!({}))];
    let reference = ("ref".to_string(), json!({}));
    let (key, hash) = contrast_key(&ppl_ratio(), 3, &reference, &terms);
    assert_eq!(key["repeats"], 3);
    assert_ne!(hash, contrast_key(&ppl_ratio(), 1, &reference, &terms).1);
}
