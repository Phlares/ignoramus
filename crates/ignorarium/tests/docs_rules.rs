use assert_cmd::Command;
use std::collections::BTreeMap;
use std::path::Path;

fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap())
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}

fn generate(out: &Path, home: &Path) {
    Command::cargo_bin("ignorarium")
        .unwrap()
        .env("IGNORARIUM_HOME", home)
        .args(["dev", "gen-rule-docs", "--out"])
        .arg(out)
        .assert()
        .success();
}

#[test]
fn rule_docs_are_up_to_date() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("docs");
    generate(&out, &tmp.path().join("home"));
    let made = tree(&out);
    let kept = tree(Path::new("../../docs/rules"));
    assert_eq!(
        made.keys().collect::<Vec<_>>(),
        kept.keys().collect::<Vec<_>>(),
        "run `ignorarium dev gen-rule-docs` and commit docs/rules"
    );
    for (name, bytes) in &made {
        assert!(
            bytes == &kept[name],
            "{name} is stale; regenerate docs/rules"
        );
    }
}

#[test]
fn a_rule_doc_has_the_rule_text_and_its_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("docs");
    generate(&out, &tmp.path().join("home"));
    let doc = std::fs::read_to_string(out.join("E-OUT-001.md")).unwrap();
    assert_eq!(
        doc,
        "# E-OUT-001\n\n\
         Severity: error\n\n\
         ## Problem\n\nAn outcome has no unit.\n\n\
         ## Fix\n\nGive every outcome a unit, for example ratio or seconds.\n\n\
         ## Why\n\nAn effect size has no meaning without its unit. Margins and power_at are stated in this unit.\n\n\
         ## Sources\n\n- <https://doi.org/10.1136/bmj.e7586>\n\n\
         ## Applies to\n\n- `outcomes[*].unit` (non-empty)\n"
    );
}

#[test]
fn a_rule_with_a_mode_gate_says_when_it_applies() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("docs");
    generate(&out, &tmp.path().join("home"));
    let gated = ignorarium::rules::builtin()
        .iter()
        .find(|r| r.when.is_some())
        .expect("v0 has a gated rule");
    let doc = std::fs::read_to_string(out.join(format!("{}.md", gated.code))).unwrap();
    assert!(doc.contains(", when "), "{doc}");
}

#[test]
fn builtin_rules_equal_the_rule_file() {
    let file = ignorarium::rules::load(Path::new("../../rules/v0.toml")).unwrap();
    let built = ignorarium::rules::builtin();
    assert_eq!(file.len(), built.len());
    for (a, b) in file.iter().zip(built) {
        assert_eq!(
            (&a.code, &a.path, a.check.name()),
            (&b.code, &b.path, b.check.name())
        );
    }
}
