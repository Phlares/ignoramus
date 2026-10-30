use ignorarium::backend::output_hash;
use ignorarium::values::{self, FILE, LIMIT};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn dir_with(text: &str) -> TempDir {
    let d = TempDir::new().unwrap();
    fs::write(d.path().join(FILE), text).unwrap();
    d
}

fn read(d: &Path, measures: &[&str]) -> Result<Vec<(String, f64)>, String> {
    values::read(d, measures).map(|(m, _)| m.into_iter().collect())
}

#[test]
fn values_must_name_every_measure_once() {
    let d = dir_with(r#"{"ppl": 12.5, "acc": 3}"#);
    let got = read(d.path(), &["ppl", "acc"]).unwrap();
    assert_eq!(got, [("acc".into(), 3.0), ("ppl".into(), 12.5)]);
    // Negative, zero and exponent forms are numbers too.
    let d = dir_with(r#"{"ppl": -1e-3, "acc": 0}"#);
    let got = read(d.path(), &["ppl", "acc"]).unwrap();
    assert_eq!(got, [("acc".into(), 0.0), ("ppl".into(), -0.001)]);
    // A key given twice is refused, whatever the second value is.
    let d = dir_with(r#"{"ppl": 1, "ppl": 1}"#);
    let e = read(d.path(), &["ppl"]).unwrap_err();
    assert!(e.contains("twice"), "{e}");
}

#[test]
fn non_finite_extra_or_missing_values_are_refused() {
    let bad = |text: &str, measures: &[&str], want: &str| {
        let d = dir_with(text);
        let e = read(d.path(), measures).unwrap_err();
        assert!(e.contains(want), "{text}: {e}");
    };
    bad(r#"{"ppl": 1}"#, &["ppl", "acc"], "missing");
    bad(r#"{"ppl": 1, "acc": 2}"#, &["ppl"], "unknown");
    bad(r#"{"ppl": "1"}"#, &["ppl"], "number");
    bad(r#"{"ppl": null}"#, &["ppl"], "number");
    bad(r#"{"ppl": true}"#, &["ppl"], "number");
    bad(r#"{"ppl": [1]}"#, &["ppl"], "number");
    bad(r#"{"ppl": 1e999}"#, &["ppl"], "number");
    bad(r#"{"ppl": NaN}"#, &["ppl"], "JSON");
    bad(r#"{"ppl": Infinity}"#, &["ppl"], "JSON");
    bad(
        r#"[1]"#,
        &["ppl"],
        "invalid type: sequence, expected an object",
    );
    bad(r#"{"ppl": 1} x"#, &["ppl"], "JSON");
    bad("", &["ppl"], "JSON");
    // A missing file is a refusal too; a file that cannot be opened says why.
    let d = TempDir::new().unwrap();
    assert_eq!(
        read(d.path(), &["ppl"]).unwrap_err(),
        "values.json is missing"
    );
    let not_a_folder = d.path().join("out");
    std::fs::write(&not_a_folder, "x").unwrap();
    let e = read(&not_a_folder, &["ppl"]).unwrap_err();
    assert!(e.starts_with("cannot open values.json: "), "{e}");
}

#[test]
fn values_must_be_covered_by_the_output_hash() {
    let d = dir_with(r#"{"ppl": 1}"#);
    let hash = output_hash(d.path()).unwrap();
    let got = values::read_covered(d.path(), &["ppl"], &hash).unwrap();
    assert_eq!(got.get("ppl"), Some(&1.0));
    // A process that rewrites the file after the hash was taken breaks the match.
    fs::write(d.path().join(FILE), r#"{"ppl": 2}"#).unwrap();
    let e = values::read_covered(d.path(), &["ppl"], &hash).unwrap_err();
    assert!(e.contains("changed"), "{e}");
    // So does a file added after the hash, even with the same values.
    let d = dir_with(r#"{"ppl": 1}"#);
    let hash = output_hash(d.path()).unwrap();
    fs::write(d.path().join("extra"), "x").unwrap();
    assert!(values::read_covered(d.path(), &["ppl"], &hash).is_err());
}

#[test]
fn reasons_quote_at_most_80_characters_of_payload_text() {
    let long = "k".repeat(5000);
    for text in [
        format!(r#"{{"{long}": 1}}"#),
        format!(r#"{{"ppl": "{long}"}}"#),
        format!(r#"{{"{long}": 1, "{long}": 1}}"#),
    ] {
        let d = dir_with(&text);
        let e = read(d.path(), &["ppl"]).unwrap_err();
        assert!(e.chars().count() < 200, "{}", e.chars().count());
    }
    // Multi-byte text is cut on a character boundary.
    let wide = "é".repeat(500);
    let d = dir_with(&format!(r#"{{"{wide}": 1}}"#));
    assert!(read(d.path(), &["ppl"]).unwrap_err().contains('…'));
}

#[test]
fn an_oversized_file_or_a_link_is_refused() {
    // Exactly the limit is read; one byte more is refused.
    let pad = |n: usize| format!(r#"{{"ppl": 1}}{}"#, " ".repeat(n));
    let fit = pad(LIMIT as usize - 10);
    assert_eq!(fit.len() as u64, LIMIT);
    let d = dir_with(&fit);
    assert_eq!(read(d.path(), &["ppl"]).unwrap(), [("ppl".into(), 1.0)]);
    let d = dir_with(&pad(LIMIT as usize - 9));
    assert!(read(d.path(), &["ppl"]).unwrap_err().contains("larger"));
    // A link, even to a good file, and a folder are not regular files.
    let d = TempDir::new().unwrap();
    let good = dir_with(r#"{"ppl": 1}"#);
    std::os::unix::fs::symlink(good.path().join(FILE), d.path().join(FILE)).unwrap();
    assert!(read(d.path(), &["ppl"]).unwrap_err().contains("regular"));
    let d = TempDir::new().unwrap();
    fs::create_dir(d.path().join(FILE)).unwrap();
    assert!(read(d.path(), &["ppl"]).unwrap_err().contains("regular"));
}
