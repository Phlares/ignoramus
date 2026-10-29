use ignorarium::diag::{Diagnostic, render_human};
use ignorarium::prior::NoPriorUse;
use ignorarium::rules::{self, CheckCtx, Rule};
use ignorarium::scan::{self, Pattern, ScanError};
use serde_json::{Value, json};
use std::path::Path;

fn kinds(s: &str) -> Vec<&'static str> {
    scan::scan_str(s, scan::builtin())
}

fn aws() -> String {
    format!("AKIA{}", "ABCDEFGHIJKLMNOP")
}

fn gh() -> String {
    format!("ghp_{}", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8")
}

fn key_header() -> &'static str {
    concat!("-----BEGIN RSA PRIV", "ATE KEY-----")
}

/// Dotted quads are written with underscores so the source holds no address.
fn ip(s: &str) -> String {
    s.replace('_', ".")
}

fn email(user: &str, host: &str) -> String {
    format!("{user}{}{host}", '@')
}

fn v0() -> &'static [Rule] {
    static RULES: std::sync::OnceLock<Vec<Rule>> = std::sync::OnceLock::new();
    RULES.get_or_init(|| rules::load("../../rules/v0.toml".as_ref()).unwrap())
}

fn run(spec: &Value) -> Vec<Diagnostic> {
    let ctx = CheckCtx {
        study_dir: ".".into(),
        prior: Box::new(NoPriorUse),
        machine_max_level: 2,
    };
    let (_, _, d) = rules::check_all(spec, v0(), &ctx);
    d.into_iter().filter(|d| d.code == "E-SEC-001").collect()
}

fn template() -> Value {
    ignorarium::spec::load(Path::new("../../templates/numeric-ablation.toml"))
        .unwrap()
        .value
}

#[test]
fn each_secret_kind_is_found() {
    assert_eq!(kinds(&format!("key {}", aws())), ["aws-key"]);
    assert_eq!(kinds(&gh()), ["github-token"]);
    assert_eq!(kinds(key_header()), ["private-key"]);
    assert_eq!(kinds("password = hunter2hunter2"), ["generic-secret"]);
    assert_eq!(kinds("API_KEY: abcd1234"), ["generic-secret"]);
    assert_eq!(kinds("api_key = sk9fj3k2l1"), ["generic-secret"]);
    assert_eq!(kinds(r#""password": "hunter2hunter2""#), ["generic-secret"]);
}

#[test]
fn generic_secret_needs_a_letter_and_a_digit_and_a_literal_value() {
    assert!(kinds("token: 100000000").is_empty());
    assert!(kinds("password = abcdefghij").is_empty());
    assert!(kinds("password = ${DB_PASSWORD}").is_empty());
    assert!(kinds("password = $DB_PASS1234").is_empty());
    assert!(kinds("password = <your-pass1>").is_empty());
    assert!(kinds("password = {pass1234}").is_empty());
    assert!(kinds("password = env:PASS1234").is_empty());
    assert_eq!(kinds("password = a1234567"), ["generic-secret"]);
}

#[test]
fn same_kind_twice_is_one_finding() {
    assert_eq!(kinds(&format!("{} {}", aws(), aws())), ["aws-key"]);
}

#[test]
fn secret_boundaries() {
    assert!(kinds("password = short").is_empty());
    assert!(kinds(&format!("AKIA{}", "ABCDEFGHIJKLMNO")).is_empty());
    assert!(kinds(&format!("ghp_{}", "a".repeat(35))).is_empty());
    assert_eq!(kinds(&format!("ghp_{}", "a".repeat(36))), ["github-token"]);
}

#[test]
fn email_rules() {
    assert_eq!(kinds(&email("someone", "gmail.com")), ["email"]);
    assert!(kinds(&email("ana", "example.org")).is_empty());
    assert!(kinds(&email("ana", "example.com")).is_empty());
    assert!(kinds(&email("ana", "mail.invalid")).is_empty());
    assert!(kinds(&email("ana", "corp.test")).is_empty());
    assert_eq!(kinds(&email("ana", "example.organization")), ["email"]);
    assert_eq!(kinds(&email("ana", "testing.io")), ["email"]);
    assert!(kinds(&email("ana", "example.net")).is_empty());
    assert_eq!(kinds(&email("someone", "example.org.evil.com")), ["email"]);
    let (a, b) = (email("x", "gmail.com"), email("a", "example.org"));
    assert_eq!(kinds(&format!("{a} example.org")), ["email"]);
    assert_eq!(kinds(&format!("{b} {a}")), ["email"]);
    assert!(kinds(&format!("{b} {b}")).is_empty());
}

#[test]
fn ipv4_rules() {
    assert_eq!(kinds(&ip("host 10_1_2_3")), ["ipv4"]);
    assert_eq!(kinds(&ip("127_0_0_1 10_1_2_3")), ["ipv4"]);
    assert_eq!(kinds(&ip("127_0_0_1,10_1_2_3")), ["ipv4"]);
    assert_eq!(kinds(&ip("127_0_0_1. 10_1_2_3")), ["ipv4"]);
    assert_eq!(kinds(&ip("host 10_1_2_3.")), ["ipv4"]);
    assert_eq!(kinds(&ip("255_255_255_255")), ["ipv4"]);
    assert_eq!(kinds(&ip("10_0_0_0")), ["ipv4"]);
    assert_eq!(kinds(&ip("127_0_0_10")), ["ipv4"]);
    assert!(kinds(&ip("127_0_0_1")).is_empty());
    assert!(kinds("bind 0.0.0.0").is_empty());
    assert!(kinds(&ip("192_0_2_7")).is_empty());
    assert!(kinds(&ip("198_51_100_7")).is_empty());
    assert!(kinds(&ip("203_0_113_7")).is_empty());
    assert!(kinds(&ip("1_2_3_4_5")).is_empty());
    assert!(kinds(&ip("999_1_1_1")).is_empty());
    assert!(kinds(&ip("256_1_1_1")).is_empty());
    assert!(kinds("version 1.2.3").is_empty());
}

#[test]
fn phone_rules() {
    assert_eq!(kinds("call (555) 010-0199"), ["phone"]);
    assert_eq!(kinds("555-010-0199"), ["phone"]);
    assert_eq!(kinds("555.010.0199"), ["phone"]);
    assert_eq!(kinds("+1 555.010.0199"), ["phone"]);
    assert_eq!(kinds("+1 555 010 0199"), ["phone"]);
    assert!(kinds("555-010-019").is_empty());
    assert!(kinds("batch sizes 128 256 1024").is_empty());
    assert!(kinds("128-256-1024").is_empty());
    assert!(kinds("seeds 100 200 3000").is_empty());
    assert!(kinds("seeds 555 010 0199").is_empty());
}

#[test]
fn hex_hashes_match_nothing() {
    let sha = "0123456789abcdef".repeat(4);
    assert!(kinds(&sha).is_empty());
    assert!(kinds(&format!("record:{sha}")).is_empty());
    let digits = "1234567890".repeat(6);
    assert!(kinds(&digits).is_empty());
}

#[test]
fn pinned_hashes_in_a_spec_pass() {
    let sha = "0123456789abcdef".repeat(4);
    let mut s = template();
    s["inputs"] = json!([{"path": "data/a.csv", "sha256": sha, "role": "selection"}]);
    s["pins"] = json!({"model": sha});
    s["design"]["variance"]["source"] = json!(format!("record:{sha}"));
    assert!(run(&s).is_empty());
}

#[test]
fn load_matches_builtin() {
    let loaded = scan::load(Path::new("../../rules/scan.toml")).unwrap();
    let pairs = |p: &[Pattern]| -> Vec<(String, String, usize)> {
        p.iter()
            .map(|p| (p.kind.clone(), p.regex.as_str().to_string(), p.allow.len()))
            .collect()
    };
    assert_eq!(loaded.len(), 7);
    assert_eq!(pairs(&loaded), pairs(scan::builtin()));
}

#[test]
fn bad_regex_names_the_kind() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("p.toml");
    std::fs::write(&f, "[[pattern]]\nkind = \"broken\"\nregex = '(['\n").unwrap();
    let Err(ScanError::BadRegex { kind, .. }) = scan::load(&f) else {
        panic!("expected a regex error");
    };
    assert_eq!(kind, "broken");
    std::fs::write(
        &f,
        "[[pattern]]\nkind = \"ok\"\nregex = 'a'\nallow = ['([']\n",
    )
    .unwrap();
    assert!(matches!(scan::load(&f), Err(ScanError::BadRegex { .. })));
    assert!(matches!(
        scan::load(&dir.path().join("none.toml")),
        Err(ScanError::Io(_))
    ));
}

#[test]
fn clean_template_has_no_finding() {
    assert!(run(&template()).is_empty());
}

#[test]
fn planted_secret_is_one_diagnostic_at_its_path() {
    let secret = aws();
    let mut s = template();
    s["question"]["population"] = json!(format!("users, key {secret}"));
    let d = run(&s);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].path, "question.population");
    assert!(
        d[0].problem.ends_with("of kind aws-key."),
        "{}",
        d[0].problem
    );
    let shown = render_human(&d);
    assert!(!shown.contains(&secret));
    assert!(!format!("{d:?}").contains(&secret));
}

#[test]
fn nested_array_paths_and_two_kinds() {
    let mut s = template();
    s["inputs"] = json!([{"path": "a"}, {"path": "b", "note": email("x", "gmail.com")}]);
    s["question"]["population"] = json!(format!("{} {}", aws(), &ip("10_1_2_3")));
    let d = run(&s);
    let got: Vec<_> = d.iter().map(|d| d.path.as_str()).collect();
    assert_eq!(
        got,
        [
            "inputs[1].note",
            "question.population",
            "question.population"
        ]
    );
}

#[test]
fn error_messages_name_the_problem() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("p.toml");
    let msg = |text: &str| {
        std::fs::write(&f, text).unwrap();
        scan::load(&f).unwrap_err().to_string()
    };
    assert!(msg("x = 1").starts_with("scan patterns are not valid: "));
    assert_eq!(
        msg("[[pattern]]
kind = \"broken\"
regex = '(['
")
        .split(':')
        .next(),
        Some("scan pattern broken")
    );
    let io = scan::load(&dir.path().join("none.toml")).unwrap_err();
    assert!(io.to_string().starts_with("cannot read scan patterns: "));
}

#[test]
fn a_named_hit_group_is_the_reported_text() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("p.toml");
    std::fs::write(
        &f,
        "[[pattern]]
kind = \"k\"
regex = '(a)(?P<hit>b+)c'
allow = ['^bb$']
",
    )
    .unwrap();
    let p = scan::load(&f).unwrap();
    assert!(scan::scan_str("abbc", &p).is_empty());
    assert_eq!(scan::scan_str("abbc abc", &p), ["k"]);
}
