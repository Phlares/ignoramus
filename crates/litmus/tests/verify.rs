use std::fs;
use std::process::{Command, Output};

fn verify(report: &str, expected: &str) -> Output {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("report.json"), report).unwrap();
    fs::write(d.path().join("expected.json"), expected).unwrap();
    Command::new(env!("CARGO_BIN_EXE_litmus"))
        .arg("verify")
        .arg("--report")
        .arg(d.path().join("report.json"))
        .arg("--expected")
        .arg(d.path().join("expected.json"))
        .output()
        .unwrap()
}

fn report(entries: &[(&str, &str)]) -> String {
    let h: Vec<String> = entries
        .iter()
        .map(|(h, v)| format!(r#"{{"hypothesis":"{h}","verdict":"{v}","basis":"x"}}"#))
        .collect();
    format!(r#"{{"study":"s","hypotheses":[{}]}}"#, h.join(","))
}

const EXPECTED: &str = r#"{"good":"pass","bad":"fail","edge":"none"}"#;

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn match_exits_zero() {
    let r = report(&[("edge", "none"), ("good", "pass"), ("bad", "fail")]);
    let out = verify(&r, EXPECTED);
    assert!(out.status.success(), "{out:?}");
    assert!(out.stderr.is_empty());
}

#[test]
fn one_difference_is_named() {
    let r = report(&[("good", "pass"), ("bad", "pass"), ("edge", "none")]);
    let out = verify(&r, EXPECTED);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("bad"), "{err}");
    assert!(err.contains("fail") && err.contains("pass"), "{err}");
    assert!(!err.contains("good") && !err.contains("edge"), "{err}");
}

#[test]
fn missing_id_is_named() {
    let out = verify(&report(&[("good", "pass"), ("bad", "fail")]), EXPECTED);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("edge") && err.contains("missing"), "{err}");
    assert!(!err.contains("good") && !err.contains("bad"), "{err}");
}

#[test]
fn extra_id_is_named() {
    let r = report(&[
        ("good", "pass"),
        ("bad", "fail"),
        ("edge", "none"),
        ("extra", "pass"),
    ]);
    let out = verify(&r, EXPECTED);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("extra"));
}

#[test]
fn malformed_input_fails_clearly() {
    let good = report(&[("good", "pass"), ("bad", "fail"), ("edge", "none")]);
    let cases = [
        ("", EXPECTED),
        ("{", EXPECTED),
        ("[]", EXPECTED),
        (r#"{"hypotheses":{}}"#, EXPECTED),
        (r#"{"hypotheses":[{"hypothesis":"a"}]}"#, EXPECTED),
        (
            r#"{"hypotheses":[{"hypothesis":"a","verdict":"pass"},{"hypothesis":"a","verdict":"pass"}]}"#,
            r#"{"a":"pass"}"#,
        ),
        (good.as_str(), ""),
        (good.as_str(), r#"["good"]"#),
        (good.as_str(), r#"{"good":1}"#),
    ];
    for (r, e) in cases {
        let out = verify(r, e);
        assert_eq!(out.status.code(), Some(1), "{r} / {e}");
        let err = stderr(&out);
        let named = err.starts_with("report: ") || err.starts_with("expected: ");
        assert!(named && !err.contains("panicked"), "{err}");
    }
}

#[test]
fn unreadable_files_fail_clearly() {
    let d = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_litmus"))
        .arg("verify")
        .arg("--report")
        .arg(d.path().join("none.json"))
        .arg("--expected")
        .arg(d.path().join("none2.json"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains("none.json") && !err.contains("panicked"),
        "{err}"
    );
}
