//! Families: `check` on a root checks each member with the full rules and the family rules.
mod common;
use assert_cmd::Command;
use common::{Lab, lab};
use ignorarium::canon::sha256_hex;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn bin(lab: &Lab) -> Command {
    let mut c = Command::cargo_bin("ignorarium").unwrap();
    c.env("IGNORARIUM_HOME", &lab.home.root);
    c
}

fn check_json(lab: &Lab, spec: &Path) -> (i32, Value) {
    let a = bin(lab).arg("check").arg(spec).arg("--json").assert();
    let out = a.get_output();
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

/// (code, path) of each diagnostic that concerns the plan's values, not its licences or pins.
fn found(v: &Value) -> Vec<(String, String)> {
    let d = v["diagnostics"].as_array().unwrap();
    d.iter()
        .map(|d| {
            let s = |k: &str| d[k].as_str().unwrap().to_string();
            (s("code"), s("path"))
        })
        .filter(|(c, _)| !c.starts_with("W-LIC") && !c.starts_with("W-HSH"))
        .collect()
}

fn has(v: &Value, code: &str, path: &str) -> bool {
    found(v).iter().any(|(c, p)| c == code && p == path)
}

fn problem_at(v: &Value, code: &str, path: &str) -> String {
    let d = v["diagnostics"].as_array().unwrap();
    let hit = d.iter().find(|d| d["code"] == code && d["path"] == path);
    hit.unwrap_or_else(|| panic!("no {code} at {path}: {v}"))["problem"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The template as study `id`, set up as `register_study` sets it up.
fn template(id: &str) -> String {
    std::fs::read_to_string("../../templates/numeric-ablation.toml")
        .unwrap()
        .replace("id = \"my-study\"", &format!("id = \"{id}\""))
        .replace("min_isolation = 1", "min_isolation = 0")
        .replace("units_per_cluster = 1", "units_per_cluster = 3")
}

/// `common::confirmatory` as study `id`: sized by `my-pilot`, 15 confirmation clusters.
fn study(id: &str) -> String {
    common::confirmatory(template(id))
}

/// A study root in the lab: the marker, the payload, and `my-pilot` finished in the box.
fn family_root(lab: &Lab) -> PathBuf {
    let root = lab.root.join("fam");
    std::fs::create_dir_all(root.join("payload")).unwrap();
    std::fs::write(root.join("ignorarium.toml"), "").unwrap();
    common::write_exec(&root.join("payload/run"), common::PAYLOAD);
    if !lab.home.study_dir("my-pilot").exists() {
        common::pilot(&lab.home, vec![common::arm_a()]);
    }
    root
}

/// `text` with each input that has no pin pinned to its file under `root`. A file that does
/// not exist yet is written first, holding its path and a newline.
fn pinned(root: &Path, text: &str) -> String {
    let mut path: Option<String> = None;
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            if let Some(p) = line.strip_prefix("path = \"") {
                path = p.split('"').next().map(str::to_string);
            }
            match (line.starts_with("sha256 = \"\""), path.take()) {
                (true, Some(p)) => {
                    let file = root.join(&p);
                    if !file.exists() {
                        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                        std::fs::write(&file, format!("{p}\n")).unwrap();
                    }
                    let sha = sha256_hex(&std::fs::read(&file).unwrap());
                    format!("sha256 = \"{sha}\"")
                }
                (_, p) => {
                    path = p;
                    line.to_string()
                }
            }
        })
        .collect();
    lines.join("\n") + "\n"
}

/// Write `text` at `rel` under `root`.
fn put(root: &Path, rel: &str, text: &str) -> PathBuf {
    let file = root.join(rel);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, text).unwrap();
    file
}

/// The root spec `spec.toml`: `text` with `[family] members = <members>`.
fn root_spec(root: &Path, text: &str, members: &[&str]) -> PathBuf {
    let list: Vec<String> = members.iter().map(|m| format!("\"{m}\"")).collect();
    let text = format!("{text}\n[family]\nmembers = [{}]\n", list.join(", "));
    put(root, "spec.toml", &text)
}

/// A root `fam` whose one member is `spec/b.toml`, the pinned `edit` of study `b`.
fn with_member(lab: &Lab, edit: impl Fn(String) -> String) -> (i32, Value) {
    let root = family_root(lab);
    put(&root, "spec/b.toml", &pinned(&root, &edit(study("b"))));
    let spec = root_spec(&root, &study("fam"), &["spec/b.toml"]);
    check_json(lab, &spec)
}

const B: &str = "family.members[0]:spec/b.toml";

#[test]
fn a_member_reusing_root_confirmation_data_passes() {
    let lab = lab();
    // Root and member list the same confirmation files.
    let (code, v) = with_member(&lab, |t| t);
    let errors: Vec<_> = (found(&v).into_iter())
        .filter(|(c, _)| c.starts_with("E-"))
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(code, 0, "{v}");
}

#[test]
fn members_are_checked_with_the_full_rules() {
    let lab = lab();
    let (code, v) = with_member(&lab, |t| {
        t.replace("role = \"confirmation\"", "role = \"calibration\"")
    });
    assert!(has(&v, "E-CNF-004", B), "{v}");
    // The problem names the member's own field.
    assert!(
        problem_at(&v, "E-CNF-004", B).starts_with("At inputs: "),
        "{v}"
    );
    assert_eq!(code, 1);
}

#[test]
fn a_member_waiting_on_a_pilot_is_a_warning_not_pending() {
    let lab = lab();
    let (code, v) = with_member(&lab, |t| t.replace("my-pilot", "later-pilot"));
    let at = "family.members[0]:spec/b.toml";
    assert!(has(&v, "W-FAM-001", at), "{v}");
    let p = problem_at(&v, "W-FAM-001", at);
    assert!(
        p.contains("At question.hypotheses[0].estimates: ") && p.contains("later-pilot"),
        "{p}"
    );
    assert!(found(&v).iter().all(|(c, _)| c != "W-REF-001"), "{v}");
    assert_eq!(v["pending"], serde_json::json!([]));
    assert_eq!(code, 0, "{v}");
}

#[test]
fn an_exploratory_member_is_refused() {
    let lab = lab();
    let root = family_root(&lab);
    put(&root, "spec/b.toml", &pinned(&root, &template("b")));
    let spec = root_spec(&root, &study("fam"), &["spec/b.toml"]);
    let (code, v) = check_json(&lab, &spec);
    assert!(has(&v, "E-FAM-001", "family.members[0]"), "{v}");
    assert_eq!(code, 1);
}

#[test]
fn a_member_on_the_test_log_is_refused() {
    let lab = lab();
    let (_, v) = with_member(&lab, |t| t.replace("test_log = false", "test_log = true"));
    assert!(has(&v, "E-FAM-001", "family.members[0]"), "{v}");
}

#[test]
fn an_exploratory_root_cannot_declare_a_family() {
    let lab = lab();
    let root = family_root(&lab);
    put(&root, "spec/b.toml", &pinned(&root, &study("b")));
    let spec = root_spec(&root, &template("fam"), &["spec/b.toml"]);
    let (_, v) = check_json(&lab, &spec);
    assert!(has(&v, "E-FAM-001", "family"), "{v}");
    assert!(!has(&v, "E-FAM-001", "family.members[0]"), "{v}");
}

#[test]
fn members_are_distinct_and_not_the_root() {
    let lab = lab();
    let root = family_root(&lab);
    put(&root, "spec/b.toml", &pinned(&root, &study("b")));
    put(&root, "spec/c.toml", &pinned(&root, &study("b")));
    let members = ["spec/b.toml", "spec/b.toml", "spec.toml", "spec/c.toml"];
    let spec = root_spec(&root, &study("fam"), &members);
    let (_, v) = check_json(&lab, &spec);
    assert!(!has(&v, "E-FAM-002", "family.members[0]"), "{v}");
    for i in 1..4 {
        assert!(
            has(&v, "E-FAM-002", &format!("family.members[{i}]")),
            "{i}: {v}"
        );
    }
    // The root, listed, declares a family: no other check reads it, so its unpinned inputs
    // are not reported again.
    assert!(!has(&v, "E-FAM-003", "family.members[2]"), "{v}");
}

#[test]
fn a_missing_member_spec_is_refused() {
    let lab = lab();
    let root = family_root(&lab);
    let spec = root_spec(&root, &study("fam"), &["spec/none.toml"]);
    let (_, v) = check_json(&lab, &spec);
    assert!(has(&v, "E-FAM-002", "family.members[0]"), "{v}");
}

#[test]
fn a_registered_study_cannot_join() {
    let lab = lab();
    common::plain(&lab, "b");
    let (_, v) = with_member(&lab, |t| t);
    assert!(has(&v, "E-FAM-002", "family.members[0]"), "{v}");
}

#[test]
fn a_member_with_its_own_family_is_refused() {
    let lab = lab();
    let (_, v) = with_member(&lab, |t| {
        format!("{t}\n[family]\nmembers = [\"spec/c.toml\"]\n")
    });
    assert!(has(&v, "E-FAM-002", "family.members[0]"), "{v}");
    // Its own check does not run, so nothing else is reported for it.
    let own: Vec<_> = (found(&v).into_iter())
        .filter(|(_, p)| p.starts_with("family.members[0]:"))
        .collect();
    assert!(own.is_empty(), "{own:?}");
}

#[test]
fn a_member_under_another_study_root_is_refused() {
    let lab = lab();
    let root = family_root(&lab);
    put(&root, "sub/ignorarium.toml", "");
    put(&root, "sub/b.toml", &pinned(&root.join("sub"), &study("b")));
    let spec = root_spec(&root, &study("fam"), &["sub/b.toml"]);
    let (_, v) = check_json(&lab, &spec);
    assert!(has(&v, "E-FAM-002", "family.members[0]"), "{v}");
}

#[test]
fn an_unpinned_or_absent_member_input_is_refused() {
    let lab = lab();
    let root = family_root(&lab);
    let model = "[[inputs]]\npath = \"data/model.bin\"\nsha256 = \"\"\nrole = \"model\"\n\
                 [inputs.licence]\nspdx = \"\"\ntype = \"unspecified\"\nsource = \"\"\ncreators = []\n\n";
    let ghost = format!(
        "[[inputs]]\npath = \"data/ghost.txt\"\nsha256 = \"{}\"\nrole = \"calibration\"\n\
         [inputs.licence]\nspdx = \"\"\ntype = \"unspecified\"\nsource = \"\"\ncreators = []\n\n",
        sha256_hex(b"never written")
    );
    let b = pinned(&root, &study("b"));
    let unpinned = b.replacen("[pins]", &format!("{model}[pins]"), 1);
    put(&root, "spec/b.toml", &unpinned);
    put(
        &root,
        "spec/c.toml",
        &pinned(&root, &study("c")).replacen("[pins]", &format!("{ghost}[pins]"), 1),
    );
    let spec = root_spec(&root, &study("fam"), &["spec/b.toml", "spec/c.toml"]);
    let (_, v) = check_json(&lab, &spec);
    assert!(has(&v, "E-FAM-003", "family.members[0]"), "{v}");
    assert!(has(&v, "E-FAM-003", "family.members[1]"), "{v}");
    let p = problem_at(&v, "E-FAM-003", "family.members[1]");
    assert!(p.contains("inputs[16]"), "{p}");
}

/// The root with selection input `data/pick.txt` holding `picked`, and member `b` with one
/// more confirmation input `data/b-extra.txt` holding `extra`.
fn selection_case(picked: &str, extra: &str) -> (i32, Value) {
    let lab = lab();
    let root = family_root(&lab);
    put(&root, "data/pick.txt", picked);
    put(&root, "data/b-extra.txt", extra);
    let input = |path: &str, role: &str| {
        format!(
            "[[inputs]]\npath = \"{path}\"\nsha256 = \"\"\nrole = \"{role}\"\nstratum = \"fiction\"\nunits = 3\n\
             [inputs.licence]\nspdx = \"\"\ntype = \"unspecified\"\nsource = \"\"\ncreators = []\n\n"
        )
    };
    let b = study("b").replacen(
        "[pins]",
        &format!("{}[pins]", input("data/b-extra.txt", "confirmation")),
        1,
    );
    put(&root, "spec/b.toml", &pinned(&root, &b));
    let fam = study("fam").replacen(
        "[pins]",
        &format!("{}[pins]", input("data/pick.txt", "selection")),
        1,
    );
    let spec = root_spec(&root, &fam, &["spec/b.toml"]);
    let out = check_json(&lab, &spec);
    drop(lab);
    out
}

#[test]
fn member_confirmation_on_root_selection_data_is_refused() {
    let (_, v) = selection_case("picked text\n", "picked text\n");
    assert!(has(&v, "E-FAM-004", "family.members[0]"), "{v}");
    let (_, v) = selection_case("picked text\n", "other text\n");
    assert!(!has(&v, "E-FAM-004", "family.members[0]"), "{v}");
}

#[test]
fn a_case_changed_copy_of_selection_data_is_refused() {
    let (_, v) = selection_case("Picked  Text\n", "picked text\n");
    assert!(has(&v, "E-FAM-004", "family.members[0]"), "{v}");
}

#[test]
fn member_data_used_by_an_outside_plan_is_refused() {
    let lab = lab();
    // An outside study registers with the data the member confirms on.
    common::register_study(
        &lab,
        "outside",
        |t| t,
        &[("data/confirm.txt", "data/confirm.txt\n")],
        common::PAYLOAD,
    );
    let (code, v) = with_member(&lab, |t| t);
    assert!(has(&v, "E-CNF-005", B), "{v}");
    assert!(
        problem_at(&v, "E-CNF-005", B).starts_with("At inputs[1].path: "),
        "{v}"
    );
    assert_eq!(code, 1);
}

#[test]
fn a_member_that_does_not_fit_the_typed_model_is_refused() {
    let lab = lab();
    // Every rule accepts this operator, and the typed model does not.
    let bad = "operators = [{ name = \"k\", type = \"int\", range = [true], acts_on = \"x\" }]";
    let (code, v) = with_member(&lab, |t| t.replace("operators = []", bad));
    assert!(has(&v, "E-TYP-001", B), "{v}");
    assert!(
        problem_at(&v, "E-TYP-001", B).starts_with("At spec: "),
        "{v}"
    );
    assert_eq!(code, 1);
}

#[test]
fn a_mistyped_family_on_an_exploratory_root_is_the_type_rule_alone() {
    let lab = lab();
    let root = family_root(&lab);
    let spec = put(
        &root,
        "spec.toml",
        &format!("family = 3\n{}", template("fam")),
    );
    let (_, v) = check_json(&lab, &spec);
    assert!(has(&v, "E-TYP-001", "family"), "{v}");
    assert!(!has(&v, "E-FAM-001", "family"), "{v}");
}

#[test]
fn members_the_context_did_not_read_fail_closed() {
    use ignorarium::rules::{self, CheckCtx};
    let dir = tempfile::tempdir().unwrap();
    let text = format!("{}\n[family]\nmembers = [\"spec/b.toml\"]\n", study("fam"));
    let value = ignorarium::spec::parse(text.as_bytes()).unwrap().value;
    let fam = |d: &[ignorarium::diag::Diagnostic]| -> Vec<(String, String)> {
        (d.iter())
            .filter(|d| d.code.starts_with("E-FAM"))
            .map(|d| (d.code.clone(), d.path.clone()))
            .collect()
    };
    let want = [("E-FAM-002".to_string(), "family.members[0]".to_string())];
    // A context built without `family::load`, and one that read another number of members.
    let none = CheckCtx::empty(dir.path());
    assert_eq!(
        fam(&rules::check_named(&value, rules::builtin(), &none)),
        want
    );
    let two = CheckCtx {
        members: vec![Err("a".into()), Err("b".into())],
        ..CheckCtx::empty(dir.path())
    };
    let d = rules::check_named(&value, rules::builtin(), &two);
    assert_eq!(fam(&d), want);
    let p = &d.iter().find(|d| d.code == "E-FAM-002").unwrap().problem;
    assert!(
        p.ends_with("This check did not read the member specs."),
        "{p}"
    );
}
