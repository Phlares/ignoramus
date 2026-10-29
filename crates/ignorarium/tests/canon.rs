use ignorarium::{canon, spec};
use serde_json::json;

#[test]
fn canonical_sorts_keys_and_hashes_known_vector() {
    let v =
        json!({"study":{"mode":"exploratory","id":"demo"},"design":{"repeats":1,"alpha":0.005}});
    let c = canon::to_canonical(&v).unwrap();
    assert_eq!(
        std::str::from_utf8(&c).unwrap(),
        r#"{"design":{"alpha":0.005,"repeats":1},"study":{"id":"demo","mode":"exploratory"}}"#
    );
    assert_eq!(
        canon::sha256_hex(&c),
        "85102a17d5c66b7514882b0435080e314008acb85018c503a0cd803968d7c412"
    );
}

#[test]
fn canonical_sorts_keys_inserted_in_reverse_order() {
    let mut m = serde_json::Map::new();
    for k in ["c", "b", "a"] {
        m.insert(k.to_string(), json!(1));
    }
    let c = canon::to_canonical(&serde_json::Value::Object(m)).unwrap();
    assert_eq!(c, br#"{"a":1,"b":1,"c":1}"#);
}

#[test]
fn crlf_and_lf_specs_hash_the_same_and_bom_parses() {
    let lf = spec::load("tests/fixtures/specs/minimal.toml".as_ref()).unwrap();
    let crlf = spec::load("tests/fixtures/specs/minimal.crlf.toml".as_ref()).unwrap();
    let bom = spec::load("tests/fixtures/specs/minimal.bom.toml".as_ref()).unwrap();
    assert!(lf.hash.starts_with("sha256:"));
    assert_eq!(lf.hash.len(), "sha256:".len() + 64);
    assert_eq!(lf.hash, crlf.hash);
    assert_eq!(lf.hash, bom.hash);
    assert_eq!(lf.value["design"]["repeats"], json!(1));
}

#[test]
fn different_specs_hash_differently() {
    let a = spec::load("tests/fixtures/specs/minimal.toml".as_ref()).unwrap();
    let b = spec::load("tests/fixtures/specs/nonfinite.toml".as_ref()).unwrap();
    assert_ne!(a.hash, b.hash);
}

#[test]
fn toml_error_reports_line_and_column() {
    match spec::load("tests/fixtures/specs/broken-syntax.toml".as_ref()) {
        Err(spec::LoadError::Toml { line, col, msg }) => {
            assert_eq!(line, 3);
            assert_eq!(col, 20);
            assert!(!msg.is_empty());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn missing_file_is_an_io_error() {
    let r = spec::load("tests/fixtures/specs/absent.toml".as_ref());
    assert!(matches!(r, Err(spec::LoadError::Io(_))), "{r:?}");
}

#[test]
fn nonfinite_floats_load_as_marker_strings() {
    let s = spec::load("tests/fixtures/specs/nonfinite.toml".as_ref()).unwrap();
    assert_eq!(s.value["x"], json!("!nonfinite:nan"));
    assert_eq!(s.value["y"], json!("!nonfinite:inf"));
    assert_eq!(s.value["z"], json!("!nonfinite:-inf"));
}

#[test]
fn datetimes_become_rfc3339_strings() {
    let dir = std::env::temp_dir().join(format!("ign-canon-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("dt.toml");
    std::fs::write(&p, "when = 2026-10-28T12:30:00Z\nlist = [1, 2.5, true]\n").unwrap();
    let s = spec::load(&p).unwrap();
    assert_eq!(s.value["when"], json!("2026-10-28T12:30:00Z"));
    assert_eq!(s.value["list"], json!([1, 2.5, true]));
    std::fs::remove_file(&p).unwrap();
}
