use ignorarium::spec::model::{Label, Mode, Spec};
use serde_json::{Value, json};

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
        "/design/variance",
        "/inputs/0",
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
    let mut v = template_value();
    v["study"]["mode"] = json!("maybe");
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
        ("/scope/stop_rule", &["budget-only", "e-process"]),
        ("/scope/end_of_run/route", &["report", "follow-on", "share"]),
        ("/design/test", &["paired-mean", "descriptive"]),
        (
            "/inputs/0/role",
            &["calibration", "selection", "confirmation", "model"],
        ),
        (
            "/inputs/0/licence/type",
            &["common", "custom", "request-form", "unspecified"],
        ),
        ("/report/publish", &["none", "zenodo-sandbox"]),
        ("/design/variance/source", &["stated", "record:abc"]),
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
