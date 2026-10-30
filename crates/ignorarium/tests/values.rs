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
    values::read(d, measures).map(|m| m.into_iter().collect())
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
    bad(r#"[1]"#, &["ppl"], "object");
    bad(r#"{"ppl": 1} x"#, &["ppl"], "JSON");
    bad("", &["ppl"], "JSON");
    // A missing file is a refusal too.
    let d = TempDir::new().unwrap();
    assert!(
        read(d.path(), &["ppl"])
            .unwrap_err()
            .contains("values.json")
    );
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
