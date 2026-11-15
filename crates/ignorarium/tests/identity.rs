use ignorarium::canon::sha256_file;
use ignorarium::identity::{self, BoxIdentity};
use ignorarium::telemetry::Reading;
use sha2::{Digest, Sha256};

#[test]
fn rules_hash_is_stable_and_covers_both_files() {
    let rules = include_str!("../../../rules/v0.toml");
    let scan = include_str!("../../../rules/scan.toml");
    let mut h = Sha256::new();
    for part in [rules, scan] {
        h.update((part.len() as u64).to_be_bytes());
        h.update(part.as_bytes());
    }
    let want = format!("sha256:{}", hex::encode(h.finalize()));
    assert_eq!(identity::rules_sha256(), want);
    assert_eq!(identity::rules_sha256(), identity::rules_sha256());
    assert_eq!(identity::current().rules_sha256, want);
}

#[test]
fn binary_hash_is_the_hash_of_the_running_test_binary() {
    let want = format!(
        "sha256:{}",
        sha256_file(&std::env::current_exe().unwrap()).unwrap()
    );
    assert_eq!(identity::current().binary_sha256, Reading::Read(want));
}

#[test]
fn same_binary_is_false_for_an_unavailable_side() {
    let a = Reading::Read("sha256:aa".to_string());
    let gone = Reading::Unavailable("no /proc".to_string());
    assert!(identity::same_binary(&a, &a.clone()));
    assert!(!identity::same_binary(
        &a,
        &Reading::Read("sha256:bb".into())
    ));
    assert!(!identity::same_binary(&a, &gone));
    assert!(!identity::same_binary(&gone, &a));
    assert!(!identity::same_binary(&gone, &gone));
}

#[test]
fn a_reading_round_trips_and_refuses_other_shapes() {
    for r in [
        Reading::Read("sha256:aa".to_string()),
        Reading::Unavailable("no /proc".to_string()),
    ] {
        let text = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Reading<String>>(&text).unwrap(), r);
    }
    for bad in ["null", "7", r#"{"unavailable": 3}"#, r#"{"other": "x"}"#] {
        assert!(
            serde_json::from_str::<Reading<String>>(bad).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn box_identity_round_trips_and_refuses_unknown_fields() {
    let id: BoxIdentity = identity::current();
    let text = serde_json::to_string(&id).unwrap();
    assert_eq!(serde_json::from_str::<BoxIdentity>(&text).unwrap(), id);
    let bad = r#"{"rules_sha256":"x","binary_sha256":"sha256:y","extra":1}"#;
    assert!(serde_json::from_str::<BoxIdentity>(bad).is_err());
}
