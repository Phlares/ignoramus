use evidence::Tail;
use evidence::claim::{Bound, Statistic};
use ignorarium::spec::model::{
    Claim, Label, Mode, PinDeviation, Reference, Spec, StatisticSpec, Timing,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const TEMPLATES: [&str; 2] = [
    "../../templates/numeric-ablation.toml",
    "../../templates/planted-spawn.toml",
];

fn template_value() -> Value {
    ignorarium::spec::load(TEMPLATES[0].as_ref()).unwrap().value
}

#[test]
fn both_templates_deserialize_into_spec() {
    for t in TEMPLATES {
        let raw = ignorarium::spec::load(t.as_ref()).unwrap();
        Spec::from_value(&raw.value).unwrap();
    }
}

#[test]
fn templates_differ_where_the_brief_says() {
    let a = Spec::from_value(&template_value()).unwrap();
    let raw = ignorarium::spec::load(TEMPLATES[1].as_ref()).unwrap();
    let b = Spec::from_value(&raw.value).unwrap();
    assert_ne!(a.study.id, b.study.id);
    assert!(!a.study.test_log && b.study.test_log);
    assert!(a.mutations.operators.is_empty() && a.mutations.triggers.is_empty());
    assert_eq!(b.mutations.operators.len(), 1);
    assert_eq!(b.mutations.operators[0].range.len(), 2);
    assert_eq!(b.mutations.triggers[0].threshold_e, 1.0);
}

#[test]
fn unknown_key_is_rejected_at_every_depth() {
    for path in [
        "/study",
        "/question",
        "/question/hypotheses/0",
        "/outcomes/0",
        "/scope",
        "/scope/end_of_run",
        "/design",
        "/design/conditions/0",
        "/pins",
        "/mutations",
        "/access",
        "/report",
        "/design/confirmation",
        "/inputs/0",
        "/inputs/1",
        "/inputs/0/licence",
        "/scope/budget",
        "/mutations/seeds",
    ] {
        let mut v = template_value();
        v.pointer_mut(path).unwrap()["surprise"] = json!(1);
        assert!(
            Spec::from_value(&v).is_err(),
            "accepted unknown key at {path}"
        );
    }
    let mut v = template_value();
    v["surprise"] = json!(1);
    assert!(Spec::from_value(&v).is_err());
}

#[test]
fn unknown_key_in_inline_table_entries_is_rejected() {
    let raw = ignorarium::spec::load(TEMPLATES[1].as_ref()).unwrap();
    for path in ["/mutations/operators/0", "/mutations/triggers/0"] {
        let mut v = raw.value.clone();
        v.pointer_mut(path).unwrap()["surprise"] = json!(1);
        assert!(
            Spec::from_value(&v).is_err(),
            "accepted unknown key at {path}"
        );
    }
    let mut v = template_value();
    v["scope"]["end_of_run"]["follow_on"] =
        json!([{ "when": "H1.pass", "spec": "a.toml", "x": 1 }]);
    assert!(Spec::from_value(&v).is_err());
}

#[test]
fn wrong_enum_value_is_rejected() {
    for path in [
        "/study/template",
        "/study/mode",
        "/study/label",
        "/outcomes/0/direction",
        "/outcomes/0/paired",
        "/scope/stop_rule",
        "/scope/end_of_run/route",
        "/design/pick",
        "/inputs/0/role",
        "/inputs/0/licence/type",
        "/report/publish",
    ] {
        let mut v = template_value();
        *v.pointer_mut(path).unwrap() = json!("bogus");
        assert!(Spec::from_value(&v).is_err(), "accepted bogus at {path}");
    }
}

#[test]
fn whole_numbers_widen_to_floats_and_fractions_do_not_fit_integers() {
    let mut v = template_value();
    v["scope"]["budget"]["cpu_hours"] = json!(1);
    v["design"]["row_seconds"] = json!(0);
    v["design"]["power"] = json!(1);
    let s = Spec::from_value(&v).unwrap();
    assert_eq!(s.scope.budget.cpu_hours, 1.0);
    assert_eq!(s.design.row_seconds, 0.0);
    assert_eq!(s.design.power, 1.0);
    v["scope"]["min_isolation"] = json!(1.5);
    assert!(Spec::from_value(&v).is_err());
}

#[test]
fn every_allowed_enum_value_deserializes() {
    let cases: [(&str, &[&str]); 11] = [
        ("/study/template", &["numeric-ablation", "planted-spawn"]),
        ("/study/mode", &["exploratory", "confirmatory"]),
        (
            "/study/label",
            &["signal", "confirmed", "replicated", "generalized"],
        ),
        ("/outcomes/0/direction", &["lower", "higher"]),
        (
            "/outcomes/0/paired",
            &["difference", "percent", "log-ratio"],
        ),
        ("/scope/stop_rule", &["budget-only", "e-process"]),
        ("/scope/end_of_run/route", &["report", "follow-on", "share"]),
        ("/design/pick", &["seeded", "first"]),
        (
            "/inputs/0/role",
            &["calibration", "selection", "confirmation", "model"],
        ),
        (
            "/inputs/0/licence/type",
            &["common", "custom", "request-form", "unspecified"],
        ),
        ("/report/publish", &["none", "zenodo-sandbox"]),
    ];
    for (path, values) in cases {
        for val in values {
            let mut v = template_value();
            *v.pointer_mut(path).unwrap() = json!(val);
            Spec::from_value(&v).unwrap_or_else(|e| panic!("{path}={val}: {e}"));
        }
    }
    let s = Spec::from_value(&{
        let mut v = template_value();
        v["study"]["label"] = json!("generalized");
        v["study"]["mode"] = json!("confirmatory");
        v
    })
    .unwrap();
    assert_eq!(s.study.label, Label::Generalized);
    assert_eq!(s.study.mode, Mode::Confirmatory);
}

#[test]
fn operator_types_and_scalars_keep_their_kind() {
    let mut v = template_value();
    v["mutations"]["operators"] = json!([
        { "name": "a", "type": "int", "range": [1, 2], "acts_on": "x" },
        { "name": "b", "type": "float", "range": [0.5, 1.5], "acts_on": "x" },
        { "name": "c", "type": "enum", "range": ["u", "v"], "acts_on": "x" },
    ]);
    let ops = Spec::from_value(&v).unwrap().mutations.operators;
    assert_eq!(format!("{:?}", ops[0].range), r#"[Int(1), Int(2)]"#);
    assert_eq!(format!("{:?}", ops[1].range), r#"[Float(0.5), Float(1.5)]"#);
    assert_eq!(format!("{:?}", ops[2].range), r#"[Text("u"), Text("v")]"#);
    v["mutations"]["operators"][0]["type"] = json!("bool");
    assert!(Spec::from_value(&v).is_err());
}

#[test]
fn bounds_follow_the_claim_shape() {
    let b = |tail, margin| Bound { tail, margin };
    assert_eq!(Claim::Below(4.0).bounds(), [b(Tail::Lower, 4.0)]);
    assert_eq!(Claim::Above(-1.5).bounds(), [b(Tail::Upper, -1.5)]);
    // Two one-sided bounds: above the low margin, then below the high one.
    assert_eq!(
        Claim::Within([-2.0, 3.0]).bounds(),
        [b(Tail::Upper, -2.0), b(Tail::Lower, 3.0)]
    );
    assert_eq!(
        StatisticSpec::Mean([-10.0, 10.0]).to_evidence(),
        Statistic::Mean {
            lo: -10.0,
            hi: 10.0
        }
    );
    assert_eq!(
        StatisticSpec::Quantile(0.9).to_evidence(),
        Statistic::Quantile(0.9)
    );
}

#[test]
fn a_hypothesis_reads_every_field_and_a_claim_is_a_one_key_table() {
    let mut v = template_value();
    v["question"]["hypotheses"][0] = json!({
        "id": "E1", "statement": "s", "rival": "r", "outcome": "ppl-ratio",
        "contrast": { "selected": 1, "arm-a": -0.5 }, "strata": ["code", "article"],
        "claim": { "within": [-4, 4] }, "statistic": { "mean": [-100.0, 100.0] },
        "power_at": 0.5, "estimates": { "from": "pilot" }, "alpha_share": 0.25
    });
    let h = Spec::from_value(&v).unwrap().question.hypotheses.remove(0);
    assert_eq!((h.id.as_str(), h.outcome.as_str()), ("E1", "ppl-ratio"));
    let weights = BTreeMap::from([("arm-a".to_string(), -0.5), ("selected".to_string(), 1.0)]);
    assert_eq!(h.contrast, weights);
    assert_eq!(h.strata, ["code", "article"]);
    assert_eq!(h.claim, Some(Claim::Within([-4.0, 4.0])));
    assert_eq!(h.statistic, Some(StatisticSpec::Mean([-100.0, 100.0])));
    assert_eq!((h.power_at, h.alpha_share), (Some(0.5), Some(0.25)));
    assert_eq!(
        h.estimates,
        Some(Reference {
            from: "pilot".into()
        })
    );
    let read = |field: &str, x: Value| {
        let mut w = v.clone();
        w["question"]["hypotheses"][0][field] = x;
        Spec::from_value(&w).map(|s| s.question.hypotheses[0].clone())
    };
    assert_eq!(
        read("claim", json!({ "below": 2 })).unwrap().claim,
        Some(Claim::Below(2.0))
    );
    assert_eq!(
        read("claim", json!({ "above": -1 })).unwrap().claim,
        Some(Claim::Above(-1.0))
    );
    assert_eq!(
        read("statistic", json!({ "quantile": 0.5 }))
            .unwrap()
            .statistic,
        Some(StatisticSpec::Quantile(0.5))
    );
    // A claim and a statistic are one-key tables; a reference holds `from` only.
    for (field, bad) in [
        ("claim", json!({ "below": 1, "above": 2 })),
        ("claim", json!({})),
        ("claim", json!({ "within": [1, 2, 3] })),
        ("claim", json!({ "near": 1 })),
        ("statistic", json!({ "quantile": 0.5, "mean": [0, 1] })),
        ("estimates", json!({ "from": "a", "x": 1 })),
    ] {
        assert!(read(field, bad.clone()).is_err(), "{field} = {bad}");
    }
    // Without a claim the hypothesis is descriptive; without strata, every unit is one group.
    let mut w = v.clone();
    let h = w["question"]["hypotheses"][0].as_object_mut().unwrap();
    for k in [
        "strata",
        "claim",
        "statistic",
        "power_at",
        "estimates",
        "alpha_share",
    ] {
        h.remove(k).unwrap();
    }
    let h = Spec::from_value(&w).unwrap().question.hypotheses.remove(0);
    assert!(h.strata.is_empty() && h.claim.is_none() && h.statistic.is_none());
    assert!(h.power_at.is_none() && h.estimates.is_none() && h.alpha_share.is_none());
    // `outcome` and `contrast` are required.
    for k in ["outcome", "contrast"] {
        let mut w = v.clone();
        w["question"]["hypotheses"][0]
            .as_object_mut()
            .unwrap()
            .remove(k);
        assert!(Spec::from_value(&w).is_err(), "{k}");
    }
}

#[test]
fn pins_list_deviations_each_with_one_of_five_timings() {
    let mut v = template_value();
    assert!(Spec::from_value(&v).unwrap().pins.deviations.is_empty());
    let timings = [
        ("before-collection", Timing::BeforeCollection),
        ("during-collection", Timing::DuringCollection),
        (
            "after-collection-before-access",
            Timing::AfterCollectionBeforeAccess,
        ),
        (
            "after-access-before-results",
            Timing::AfterAccessBeforeResults,
        ),
        ("after-results", Timing::AfterResults),
    ];
    for (text, timing) in timings {
        v["pins"]["deviations"] = json!([{ "from": "pilot", "reason": "r", "timing": text }]);
        assert_eq!(
            Spec::from_value(&v).unwrap().pins.deviations,
            [PinDeviation {
                from: "pilot".into(),
                reason: "r".into(),
                timing
            }],
            "{text}"
        );
        assert_eq!(serde_json::to_value(timing).unwrap(), json!(text));
    }
    for bad in [
        json!({ "from": "pilot", "reason": "r", "timing": "later" }),
        json!({ "from": "pilot", "reason": "r", "timing": "before-collection", "x": 1 }),
        json!({ "from": "pilot", "reason": "r" }),
        json!({ "from": "pilot", "timing": "after-results" }),
        json!({ "reason": "r", "timing": "after-results" }),
    ] {
        v["pins"]["deviations"] = json!([bad.clone()]);
        assert!(Spec::from_value(&v).is_err(), "{bad}");
    }
}
