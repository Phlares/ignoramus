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
    let lf = spec::load(fx("minimal.toml").as_path()).unwrap();
    let crlf = spec::load(fx("minimal.crlf.toml").as_path()).unwrap();
    let bom = spec::load(fx("minimal.bom.toml").as_path()).unwrap();
    assert!(lf.hash.starts_with("sha256:"));
    assert_eq!(lf.hash.len(), "sha256:".len() + 64);
    assert_eq!(lf.hash, crlf.hash);
    assert_eq!(lf.hash, bom.hash);
    assert_eq!(lf.value["design"]["repeats"], json!(1));
}

#[test]
fn different_specs_hash_differently() {
    let a = spec::load(fx("minimal.toml").as_path()).unwrap();
    let b = spec::load(fx("nonfinite.toml").as_path()).unwrap();
    assert_ne!(a.hash, b.hash);
}

#[test]
fn toml_error_reports_line_and_column() {
    match spec::load(fx("broken-syntax.toml").as_path()) {
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
    let r = spec::load(fx("absent.toml").as_path());
    assert!(matches!(r, Err(spec::LoadError::Io(_))), "{r:?}");
}

#[test]
fn nonfinite_floats_load_as_marker_strings() {
    let s = spec::load(fx("nonfinite.toml").as_path()).unwrap();
    assert_eq!(s.value["x"], json!("!nonfinite:nan"));
    assert_eq!(s.value["y"], json!("!nonfinite:inf"));
    assert_eq!(s.value["z"], json!("!nonfinite:-inf"));
}

#[test]
fn arrays_and_scalars_convert() {
    let s = load_text(
        b"list = [1, 2.5, true]
",
    )
    .unwrap();
    assert_eq!(s.value["list"], json!([1, 2.5, true]));
}

fn fx(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/specs")
        .join(name)
}

fn load_text(bytes: &[u8]) -> Result<spec::RawSpec, spec::LoadError> {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("s.toml");
    std::fs::write(&p, bytes).unwrap();
    spec::load(&p)
}

fn toml_pos(name: &str) -> (usize, usize) {
    match spec::load(&fx(name)) {
        Err(spec::LoadError::Toml { line, col, .. }) => (line, col),
        other => panic!("{other:?}"),
    }
}

#[test]
fn fixtures_keep_their_special_bytes() {
    let crlf = std::fs::read(fx("minimal.crlf.toml")).unwrap();
    assert!(crlf.windows(2).any(|w| w == b"\r\n"));
    let bom = std::fs::read(fx("minimal.bom.toml")).unwrap();
    assert_eq!(bom[..3], [0xEF, 0xBB, 0xBF]);
    let mcrlf = std::fs::read(fx("multiline.crlf.toml")).unwrap();
    assert!(mcrlf.windows(2).any(|w| w == b"\r\n"));
    let bcrlf = std::fs::read(fx("broken-syntax.crlf.toml")).unwrap();
    assert!(bcrlf.windows(2).any(|w| w == b"\r\n"));
}

#[test]
fn integer_and_float_hash_differently_and_zero_sign_is_kept() {
    let canon_of = |src: &str| {
        let s = load_text(src.as_bytes()).unwrap();
        (
            String::from_utf8(canon::to_canonical(&s.value).unwrap()).unwrap(),
            s.hash,
        )
    };
    let (i, ih) = canon_of("a = 1\n");
    let (f, fh) = canon_of("a = 1.0\n");
    assert_eq!(i, r#"{"a":1}"#);
    assert_eq!(f, r#"{"a":1.0}"#);
    assert_ne!(ih, fh);
    assert_eq!(canon_of("a = 0.0\n").0, r#"{"a":0.0}"#);
    assert_eq!(canon_of("a = -0.0\n").0, r#"{"a":-0.0}"#);
    assert_ne!(canon_of("a = 0.0\n").1, canon_of("a = -0.0\n").1);
}

#[test]
fn error_columns_count_characters() {
    assert_eq!(toml_pos("broken-multibyte-col.toml"), (1, 14));
    assert_eq!(toml_pos("broken-multibyte-line.toml"), (3, 9));
}

#[test]
fn crlf_error_position_matches_lf_twin() {
    assert_eq!(
        toml_pos("broken-syntax.crlf.toml"),
        toml_pos("broken-syntax.toml")
    );
    assert_eq!(toml_pos("broken-syntax.crlf.toml"), (3, 20));
}

#[test]
fn invalid_utf8_is_an_error_with_offset() {
    match load_text(b"a = \"ok\"\nb = \"\xff\"\n") {
        Err(spec::LoadError::Utf8 { offset }) => assert_eq!(offset, 14),
        other => panic!("{other:?}"),
    }
}

#[test]
fn crlf_multiline_string_hashes_like_lf() {
    let lf = spec::load(&fx("multiline.toml")).unwrap();
    let crlf = spec::load(&fx("multiline.crlf.toml")).unwrap();
    assert_eq!(lf.value["s"], json!("line one\nline two\n"));
    assert_eq!(lf.hash, crlf.hash);
}

#[test]
fn datetimes_hash_as_written() {
    let v = |src: &str| load_text(src.as_bytes()).unwrap().value["d"].clone();
    assert_eq!(
        v("d = 2026-10-28T12:30:00Z\n"),
        json!("2026-10-28T12:30:00Z")
    );
    assert_eq!(
        v("d = 2026-10-28T12:30:00+00:00\n"),
        json!("2026-10-28T12:30:00+00:00")
    );
    assert_eq!(
        v("d = 2026-10-28 12:30:00Z\n"),
        json!("2026-10-28T12:30:00Z")
    );
    assert_eq!(v("d = 2026-10-28T12:30:00\n"), json!("2026-10-28T12:30:00"));
    assert_eq!(v("d = 2026-10-28\n"), json!("2026-10-28"));
    assert_eq!(v("d = 12:30:00\n"), json!("12:30:00"));
}
