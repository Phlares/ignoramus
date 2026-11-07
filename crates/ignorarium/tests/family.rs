//! Families: `check` on a root checks each member with the full rules and the family rules;
//! `register` on a root locks its members and reserves their stream slots.
mod common;
use assert_cmd::Command;
use common::{Lab, lab};
use ignorarium::canon::sha256_hex;
use ignorarium::cli::register;
use ignorarium::derived::Register;
use ignorarium::family;
use ignorarium::planlog::store::{Leaf, PlanLog};
use ignorarium::prior::file_hashes;
use ignorarium::stream::{Log, LogName};
use ignorarium::{inputs, keys};
use serde_json::{Value, json};
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

/// An unpinned input of 3 units in stratum fiction, as spec text.
fn input(path: &str, role: &str) -> String {
    format!(
        "[[inputs]]\npath = \"{path}\"\nsha256 = \"\"\nrole = \"{role}\"\nstratum = \"fiction\"\nunits = 3\n\
         [inputs.licence]\nspdx = \"\"\ntype = \"unspecified\"\nsource = \"\"\ncreators = []\n\n"
    )
}

/// `text` with `inputs`, as spec text, after its other inputs.
fn adding(text: String, inputs: &str) -> String {
    text.replacen("[pins]", &format!("{inputs}[pins]"), 1)
}

/// The root with selection input `data/pick.txt` holding `picked`, and member `b` with one
/// more confirmation input `data/b-extra.txt` holding `extra`.
fn selection_case(picked: &str, extra: &str) -> (i32, Value) {
    let lab = lab();
    let root = family_root(&lab);
    put(&root, "data/pick.txt", picked);
    put(&root, "data/b-extra.txt", extra);
    let b = adding(study("b"), &input("data/b-extra.txt", "confirmation"));
    put(&root, "spec/b.toml", &pinned(&root, &b));
    let fam = adding(study("fam"), &input("data/pick.txt", "selection"));
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

// ---- registering a root ----

/// Register the spec at `spec` with `register::run`, approving its hash: the exit code, and
/// the output followed by the refusal, if any.
fn try_register(lab: &Lab, spec: &Path) -> (i32, String) {
    let hash = ignorarium::spec::load(spec).unwrap().hash;
    let typed = format!("{}\n", &hash["sha256:".len()..][..8]);
    let mut out = Vec::new();
    let mut answer = std::io::Cursor::new(typed.into_bytes());
    let printed = |out: &[u8]| String::from_utf8_lossy(out).into_owned();
    match register::run(spec, &lab.home, &mut answer, &mut out, true) {
        Ok(done) => (done.code, printed(&out)),
        Err(e) => (e.code, format!("{}\n{}", printed(&out), e.msg)),
    }
}

/// `try_register`, which must succeed.
fn register(lab: &Lab, spec: &Path) {
    let (code, out) = try_register(lab, spec);
    assert_eq!(code, 0, "{out}");
}

/// The data member `b` of `registered_family` holds and the root does not.
const EXTRA: &str = "member data\n";

/// Root `fam` with members `spec/b.toml` and `spec/c.toml`, registered. Member `b` has one
/// more confirmation input, `data/b-extra.txt`, which holds `EXTRA`. Returns the study root.
fn registered_family(lab: &Lab) -> PathBuf {
    registered_family_with(lab, |t| t)
}

/// `registered_family` with `edit` applied to the root and to each member.
fn registered_family_with(lab: &Lab, edit: impl Fn(String) -> String) -> PathBuf {
    let root = family_root(lab);
    put(&root, "data/b-extra.txt", EXTRA);
    let b = adding(study("b"), &input("data/b-extra.txt", "confirmation"));
    put(&root, "spec/b.toml", &pinned(&root, &edit(b)));
    put(&root, "spec/c.toml", &pinned(&root, &edit(study("c"))));
    let spec = root_spec(&root, &edit(study("fam")), &["spec/b.toml", "spec/c.toml"]);
    register(lab, &spec);
    root
}

fn register_of(lab: &Lab, id: &str) -> Register {
    Register::of(&common::kind_of(lab, id, "register")[0]).unwrap()
}

fn plan_leaves(lab: &Lab) -> Vec<Leaf> {
    let log = PlanLog::open(&lab.home.root.join("planlog")).unwrap();
    log.leaves().unwrap()
}

/// The `inputs` of the spec at `rel` under `root`, as written.
fn inputs_of(root: &Path, rel: &str) -> Vec<Value> {
    let raw = ignorarium::spec::load(&root.join(rel)).unwrap();
    raw.value["inputs"].as_array().unwrap().clone()
}

#[test]
fn register_writes_the_lock() {
    let lab = lab();
    let root = registered_family(&lab);
    let reg = register_of(&lab, "fam");
    let lock = (reg.family.clone()).expect("a root's register record holds its lock");
    let ids: Vec<&str> = lock.members.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["b", "c"]);
    for (m, rel) in lock.members.iter().zip(["spec/b.toml", "spec/c.toml"]) {
        assert_eq!(m.spec, rel);
        assert_eq!(
            m.spec_hash,
            ignorarium::spec::load(&root.join(rel)).unwrap().hash
        );
        assert_eq!(m.payload, reg.payload);
        let want: Vec<Value> = (inputs_of(&root, rel).iter())
            .map(|i| json!({ "path": i["path"], "role": i["role"], "sha256": i["sha256"] }))
            .collect();
        assert_eq!(serde_json::to_value(&m.inputs).unwrap(), json!(want));
        assert!(m.inputs.iter().all(|i| i.sha256.len() == 64), "{m:?}");
    }
    assert!(
        lock.members[0]
            .inputs
            .iter()
            .any(|i| i.path == "data/b-extra.txt")
    );
    // A plain study's record carries no lock, and no key for one.
    common::plain(&lab, "plain");
    let body = &common::kind_of(&lab, "plain", "register")[0].body;
    assert!(body.get("family").is_none(), "{body}");
}

#[test]
fn register_imports_every_member_input() {
    let lab = lab();
    let root = registered_family(&lab);
    let store = ignorarium::store::Store::open(&lab.home.root).unwrap();
    for rel in ["spec/b.toml", "spec/c.toml"] {
        for i in inputs_of(&root, rel) {
            let pin = i["sha256"].as_str().unwrap();
            assert!(
                matches!(
                    inputs::stored(&store, pin),
                    Some(inputs::Found::Stored { .. })
                ),
                "{rel}: {}",
                i["path"]
            );
        }
    }
}

#[test]
fn the_family_leaf_follows_the_root_plan_leaf() {
    let lab = lab();
    let root = registered_family(&lab);
    let reg = register_of(&lab, "fam");
    let leaves = plan_leaves(&lab);
    assert_eq!(leaves.len() as u64, reg.leaf + 2);
    let f = &leaves[reg.leaf as usize + 1];
    assert_eq!(family::LEAF_KIND, "family");
    assert_eq!(family::leaf_label("fam"), "family:fam");
    assert_eq!(
        (f.kind.as_str(), f.label.as_str()),
        (family::LEAF_KIND, "family:fam")
    );
    let lock = reg.family.unwrap();
    assert!(lock.hash().starts_with("sha256:"));
    assert_eq!(f.commitment, Some(lock.hash()));
    assert_eq!((&f.shape, &f.mode), (&None, &None));
    // Each member's confirmation and reserve data, once.
    let read = |i: &Value| std::fs::read(root.join(i["path"].as_str().unwrap())).unwrap();
    let mut want: Vec<(String, String)> = (["spec/b.toml", "spec/c.toml"].iter())
        .flat_map(|rel| inputs_of(&root, rel))
        .filter(|i| matches!(i["role"].as_str(), Some("confirmation" | "reserve")))
        .map(|i| file_hashes(&read(&i)))
        .collect();
    want.sort();
    want.dedup();
    let mut got = f.data_hashes.clone();
    got.sort();
    assert_eq!(got, want);
    assert_eq!(f.data_hashes.len(), want.len());
    assert!(want.contains(&file_hashes(EXTRA.as_bytes())));
}

/// A study root `out` holding a confirmatory study `out`: `edit` of `study("out")`, with
/// `data/confirm.txt` holding `confirm`. Returns its spec.
fn outsider(lab: &Lab, confirm: &str, edit: impl Fn(String) -> String) -> PathBuf {
    let root = lab.root.join("out");
    put(&root, "data/confirm.txt", confirm);
    put(&root, "spec.toml", &edit(study("out")))
}

#[test]
fn an_outsider_cannot_confirm_on_member_data() {
    let lab = lab();
    registered_family(&lab);
    // Only member b lists this data; the family leaf holds it.
    let (code, v) = check_json(&lab, &outsider(&lab, EXTRA, |t| t));
    let p = problem_at(&v, "E-CNF-005", "inputs[1].path");
    assert!(p.ends_with("The earlier entry is leaf 1 (family)."), "{p}");
    assert_eq!(code, 1);
}

#[test]
fn an_outsider_may_select_on_member_data() {
    let lab = lab();
    registered_family(&lab);
    put(&lab.root.join("out"), "data/pick.txt", EXTRA);
    let spec = outsider(&lab, "fresh data\n", |t| {
        adding(t, &input("data/pick.txt", "selection"))
    });
    let (_, v) = check_json(&lab, &spec);
    assert!(found(&v).iter().all(|(c, _)| c != "E-CNF-005"), "{v}");
}

/// `flat("99")` of the run tests: 100 for the reference, 99 for `arm-a`.
const FLAT_99: &str = "#!/bin/sh\ncase \"$2\" in exact) v=100;; *) v=99;; esac\nprintf '{\"ppl\": %s}' \"$v\" > \"$IGNORARIUM_OUT/values.json\"\n";

#[test]
fn a_root_release_does_not_free_member_data() {
    use ignorarium::backend::level0::Level0;
    use ignorarium::runner::{RunStatus, prepare_with, run};
    const RESERVE: usize = 32;
    let lab = lab();
    common::resize_pilot(&lab.home);
    let root = family_root(&lab);
    common::write_exec(&root.join("payload/run"), FLAT_99);
    let files = common::reserve_files(RESERVE);
    for (p, t) in &files {
        put(&root, p, t);
    }
    // Root and member re-size from the same reserve.
    let resizing = |id: &str| common::resize_plan(RESERVE)(template(id));
    put(&root, "spec/b.toml", &pinned(&root, &resizing("b")));
    let spec = root_spec(&root, &resizing("fam"), &["spec/b.toml"]);
    register(&lab, &spec);
    let plan = prepare_with(&lab.home, "fam", &root, vec![Box::new(Level0)]).unwrap();
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let release = (plan_leaves(&lab).into_iter())
        .find(|l| l.kind == "release")
        .expect("the root releases the reserve data it did not use");
    let (exact, _) = &release.data_hashes[0];
    let (_, freed) = (files.iter())
        .find(|(_, t)| &file_hashes(t.as_bytes()).0 == exact)
        .unwrap();
    // The root's release frees the data from its own plan leaf, not from the member's lock.
    let (_, v) = check_json(&lab, &outsider(&lab, freed, |t| t));
    let p = problem_at(&v, "E-CNF-005", "inputs[1].path");
    assert!(p.ends_with("(family)."), "{p}");
}

#[test]
fn a_study_in_another_family_cannot_join() {
    let lab = lab();
    let root = registered_family(&lab);
    let text = format!("{}\n[family]\nmembers = [\"spec/b.toml\"]\n", study("fam2"));
    let (_, v) = check_json(&lab, &put(&root, "spec2.toml", &text));
    let p = problem_at(&v, "E-FAM-002", "family.members[0]");
    assert!(
        p.ends_with("It is already locked in a registered family."),
        "{p}"
    );
}

/// Open `n` more plan stream slots, for studies outside any family.
fn take_slots(lab: &Lab, n: usize) {
    let key = keys::load_or_create(&lab.home).unwrap();
    let log = Log::of(&lab.home, LogName::Plan);
    for i in 0..n {
        log.open(&key, &format!("other-{i}"), 0.0, 0).unwrap();
    }
}

#[test]
fn a_root_whose_members_do_not_fit_is_refused() {
    let lab = lab();
    let root = family_root(&lab);
    let members = ["spec/b.toml", "spec/c.toml", "spec/d.toml"];
    for (rel, id) in members.iter().zip(["b", "c", "d"]) {
        put(&root, rel, &pinned(&root, &study(id)));
    }
    let spec = root_spec(&root, &study("fam"), &members);
    // Slot 7 leaves 3 slots for 3 members.
    take_slots(&lab, 6);
    let (code, v) = check_json(&lab, &spec);
    assert!(!has(&v, "E-FAM-005", "family"), "{v}");
    assert_eq!(code, 0, "{v}");
    // Slot 8 leaves 2.
    take_slots(&lab, 1);
    let (code, v) = check_json(&lab, &spec);
    let p = problem_at(&v, "E-FAM-005", "family");
    assert!(
        p.ends_with(
            "Slot 8 leaves 2 slots after it, fewer than the 3 slots needed: 3 for this family's members and 0 already reserved."
        ),
        "{p}"
    );
    assert!(!has(&v, "E-STR-001", "study.mode"), "{v}");
    assert_eq!(code, 1);
}

#[test]
fn a_plan_that_would_take_a_reserved_slot_is_refused() {
    let lab = lab();
    let root = registered_family(&lab);
    let spec = outsider(&lab, "fresh data\n", |t| t);
    // The root holds slot 1. Slot 8 leaves 2 slots, for b and c.
    take_slots(&lab, 6);
    let (_, v) = check_json(&lab, &spec);
    assert!(!has(&v, "E-STR-001", "study.mode"), "{v}");
    take_slots(&lab, 1);
    let (code, v) = check_json(&lab, &spec);
    let p = problem_at(&v, "E-STR-001", "study.mode");
    assert!(
        p.ends_with(
            "Slot 9 leaves 1 slot after it, fewer than the 2 slots reserved for family members not yet registered."
        ),
        "{p}"
    );
    assert_eq!(code, 1);
    // A member's own slot is among the reserved ones.
    let (_, v) = check_json(&lab, &root.join("spec/b.toml"));
    assert!(!has(&v, "E-STR-001", "study.mode"), "{v}");
}

#[test]
fn check_shows_slots_left_and_reserved() {
    let lab = lab();
    registered_family(&lab);
    let spec = outsider(&lab, "fresh data\n", |t| t);
    let (_, v) = check_json(&lab, &spec);
    assert_eq!(v["plan_stream"], json!({ "slots_left": 9, "reserved": 2 }));
    let a = bin(&lab).arg("check").arg(&spec).assert();
    let out = String::from_utf8(a.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains("\nstream: slot 2, level 0.005\nplan stream: slots left 9, reserved 2\n"),
        "{out}"
    );
    // The test stream reserves nothing.
    let spec = outsider(&lab, "fresh data\n", |t| {
        t.replace("test_log = false", "test_log = true")
    });
    let (_, v) = check_json(&lab, &spec);
    assert_eq!(v["plan_stream"], Value::Null);
    assert!(v["stream"].is_object(), "{v}");
}

#[test]
fn a_family_on_the_test_log_reserves_no_plan_slot() {
    let lab = lab();
    registered_family_with(&lab, |t| t.replace("test_log = false", "test_log = true"));
    // Slot 10 leaves no slot after it: two reserved members would refuse it.
    take_slots(&lab, 9);
    let (code, v) = check_json(&lab, &outsider(&lab, "fresh data\n", |t| t));
    assert_eq!(v["plan_stream"], json!({ "slots_left": 1, "reserved": 0 }));
    assert!(!has(&v, "E-STR-001", "study.mode"), "{v}");
    assert_eq!(code, 0, "{v}");
}

#[test]
fn a_study_locked_by_a_test_log_family_cannot_join_a_plan_log_family() {
    let lab = lab();
    let root = registered_family_with(&lab, |t| t.replace("test_log = false", "test_log = true"));
    let text = format!("{}\n[family]\nmembers = [\"spec/b.toml\"]\n", study("fam2"));
    let (_, v) = check_json(&lab, &put(&root, "spec2.toml", &text));
    let p = problem_at(&v, "E-FAM-002", "family.members[0]");
    assert!(
        p.contains("It is already locked in a registered family."),
        "{p}"
    );
}

// ---- members of a registered family ----

/// Run study `id` from `root` at level 0 to its end.
fn run_to_end(lab: &Lab, id: &str, root: &Path) {
    use ignorarium::backend::level0::Level0;
    use ignorarium::runner::{RunStatus, prepare_with, run};
    let plan = prepare_with(&lab.home, id, root, vec![Box::new(Level0)]).unwrap();
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
}

/// The diagnostics of `v` with an `E-` code.
fn errors(v: &Value) -> Vec<(String, String)> {
    (found(v).into_iter())
        .filter(|(c, _)| c.starts_with("E-"))
        .collect()
}

#[test]
fn a_member_reuses_root_confirmation_data_end_to_end() {
    let lab = lab();
    let root = registered_family(&lab);
    run_to_end(&lab, "fam", &root);
    // Member b confirms on the root's confirmation data, and on its own.
    let b = root.join("spec/b.toml");
    let (code, v) = check_json(&lab, &b);
    assert!(errors(&v).is_empty(), "{v}");
    assert_eq!(code, 0, "{v}");
    register(&lab, &b);
    run_to_end(&lab, "b", &root);
    let slot = register_of(&lab, "b").alpha.unwrap().slot;
    let key = keys::load(&lab.home).unwrap().unwrap().verifying_key();
    let closed = Log::of(&lab.home, LogName::Plan).closed(&key, "b", slot);
    assert!(closed.unwrap(), "b closes its own slot {slot}");
}

#[test]
fn a_member_after_the_lock_ignores_outside_uses() {
    let lab = lab();
    let root = registered_family(&lab);
    // An exploratory outsider runs on member b's own data after the lock.
    common::register_study(
        &lab,
        "outside",
        |t| t,
        &[("data/confirm.txt", EXTRA)],
        common::PAYLOAD,
    );
    run_to_end(&lab, "outside", &common::work(&lab, "outside"));
    let b = root.join("spec/b.toml");
    let (_, v) = check_json(&lab, &b);
    assert!(errors(&v).is_empty(), "{v}");
    register(&lab, &b);
}

/// The index of the input at `path` in the spec at `rel` under `root`.
fn input_index(root: &Path, rel: &str, path: &str) -> usize {
    (inputs_of(root, rel).iter())
        .position(|i| i["path"] == path)
        .unwrap()
}

#[test]
fn a_changed_member_is_refused() {
    let lab = lab();
    let root = registered_family(&lab);
    let b = root.join("spec/b.toml");
    let locked = std::fs::read_to_string(&b).unwrap();
    let refused = |what: &str| {
        let (code, v) = check_json(&lab, &b);
        let p = problem_at(&v, "E-FAM-006", "study.id");
        assert!(p.contains(what), "{what}: {p}");
        assert!(p.contains("new id") && p.contains("Restore"), "{p}");
        assert_eq!(code, 1);
        p
    };
    // The spec.
    std::fs::write(
        &b,
        locked.replace("row_seconds = 60.0", "row_seconds = 61.0"),
    )
    .unwrap();
    let p = refused("Its spec hash is ");
    assert!(!p.contains("payload"), "{p}");
    let (code, out) = try_register(&lab, &b);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("E-FAM-006"), "{out}");
    std::fs::write(&b, &locked).unwrap();
    let (code, v) = check_json(&lab, &b);
    assert_eq!((code, errors(&v)), (0, vec![]), "{v}");
    // The payload.
    let run = root.join("payload/run");
    let payload = std::fs::read_to_string(&run).unwrap();
    std::fs::write(&run, format!("{payload}# edited\n")).unwrap();
    refused("Its payload hash is ");
    std::fs::write(&run, &payload).unwrap();
    // An input's pin, with new bytes behind it.
    put(&root, "data/b-extra.txt", "changed member data\n");
    let old = sha256_hex(EXTRA.as_bytes());
    let new = sha256_hex(b"changed member data\n");
    std::fs::write(&b, locked.replace(&old, &new)).unwrap();
    let k = input_index(&root, "spec/b.toml", "data/b-extra.txt");
    let p = refused(&format!(
        "Its inputs[{k}].sha256 is {new}; the lock holds {old}."
    ));
    assert!(!p.contains("spec hash"), "{p}");
}

#[test]
fn check_names_the_family() {
    let lab = lab();
    let root = family_root(&lab);
    put(&root, "spec/b.toml", &pinned(&root, &study("b")));
    put(&root, "spec/c.toml", &pinned(&root, &study("c")));
    let spec = root_spec(&root, &study("fam"), &["spec/b.toml", "spec/c.toml"]);
    let (_, v) = check_json(&lab, &spec);
    let line = "family: 2 members; plan stream slots left 10, reserved 0";
    assert_eq!(v["family"], line, "{v}");
    let a = bin(&lab).arg("check").arg(&spec).assert();
    let out = String::from_utf8(a.get_output().stdout.clone()).unwrap();
    assert!(out.contains(&format!("\n{line}\n")), "{out}");
    register(&lab, &spec);
    // Before the root has run.
    let b = root.join("spec/b.toml");
    let (_, v) = check_json(&lab, &b);
    assert_eq!(v["family"], "member of family fam; slot reserved", "{v}");
    let a = bin(&lab).arg("check").arg(&b).assert();
    let out = String::from_utf8(a.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains("\nmember of family fam; slot reserved\n"),
        "{out}"
    );
    let (_, v) = check_json(&lab, &outsider(&lab, "fresh data\n", |t| t));
    assert_eq!(v["family"], Value::Null, "{v}");
}

/// A register record for root `fam` in `log` whose plan leaf is `leaf`, holding `lock`, signed
/// into the box by hand.
fn plant_root(lab: &Lab, log: &str, leaf: u64, lock: Option<&family::Lock>) {
    let zeros = "0".repeat(64);
    let body = json!({
        "spec_hash": format!("sha256:{zeros}"),
        "leaf": leaf,
        "log": log,
        "defaults": {},
        "root_id": zeros,
        "payload": zeros,
        "inputs": [],
        "resolved": [],
        "family": lock,
        "alpha": null,
    });
    common::plant(&lab.home, "fam", &[("register", body)]);
    assert!(Register::of(&common::kind_of(lab, "fam", "register")[0]).is_some());
}

/// Append a leaf of `kind` with `label`, `commitment` and `data` to the box's log `log`.
fn append_leaf(
    lab: &Lab,
    log: &str,
    kind: &str,
    label: &str,
    commitment: Option<&str>,
    data: &[(String, String)],
) {
    let key = keys::load_or_create(&lab.home).unwrap();
    let leaf = Leaf {
        kind: kind.into(),
        commitment: commitment.map(str::to_string),
        shape: None,
        mode: None,
        data_hashes: data.to_vec(),
        label: label.into(),
        ts: "2026-11-06T00:00:00Z".into(),
    };
    let log = PlanLog::open(&lab.home.root.join(log)).unwrap();
    log.append(leaf, &key).unwrap();
}

/// A study root with member spec `spec/b.toml`, the pinned `edit` of study `b`: the study root
/// and the spec's path.
fn lone_member(lab: &Lab, edit: impl Fn(String) -> String) -> (PathBuf, PathBuf) {
    let root = family_root(lab);
    let b = put(&root, "spec/b.toml", &pinned(&root, &edit(study("b"))));
    (root, b)
}

/// The data of each confirmation input of `spec/b.toml` under `root`.
fn confirmation_data(root: &Path) -> Vec<(String, String)> {
    (inputs_of(root, "spec/b.toml").iter())
        .filter(|i| i["role"] == "confirmation")
        .map(|i| file_hashes(&std::fs::read(root.join(i["path"].as_str().unwrap())).unwrap()))
        .collect()
}

/// A lock that lists member `b` at `spec/b.toml`.
fn lock_of_b(b: &Path) -> family::Lock {
    family::Lock {
        members: vec![family::Member {
            id: "b".into(),
            spec: "spec/b.toml".into(),
            spec_hash: ignorarium::spec::load(b).unwrap().hash,
            payload: "0".repeat(64),
            inputs: vec![],
        }],
    }
}

/// The problem of b's E-CNF-005 at its input `data/confirm.txt`, which must refuse.
fn confirm_refused(lab: &Lab, root: &Path, b: &Path) -> String {
    let (code, v) = check_json(lab, b);
    let k = input_index(root, "spec/b.toml", "data/confirm.txt");
    assert_eq!(code, 1, "{v}");
    problem_at(&v, "E-CNF-005", &format!("inputs[{k}].path"))
}

#[test]
fn a_root_without_its_family_leaf_fails_member_checks_closed() {
    // A lock that lists b, and no family leaf.
    let lab = lab();
    let (_, b) = lone_member(&lab, |t| t);
    plant_root(&lab, "planlog", 0, Some(&lock_of_b(&b)));
    let (code, v) = check_json(&lab, &b);
    let p = problem_at(&v, "E-FAM-006", "study.id");
    assert!(
        p.contains("cannot be read") && p.contains("no family leaf"),
        "{p}"
    );
    assert_eq!(code, 1);
    // A family leaf whose root's record holds no lock.
    let lab = common::lab();
    let (root, b) = lone_member(&lab, |t| t);
    let data = confirmation_data(&root);
    append_leaf(&lab, "planlog", "plan", "", Some("sha256:plan"), &data);
    let label = family::leaf_label("fam");
    append_leaf(
        &lab,
        "planlog",
        family::LEAF_KIND,
        &label,
        Some("sha256:x"),
        &data,
    );
    plant_root(&lab, "planlog", 0, None);
    let p = confirm_refused(&lab, &root, &b);
    assert!(p.contains("The family records cannot be read: "), "{p}");
}

#[test]
fn a_family_leaf_without_a_commitment_fails_closed() {
    // The root's record holds no lock, and the leaf no commitment: they must not match.
    let lab = lab();
    let (root, b) = lone_member(&lab, |t| t);
    let data = confirmation_data(&root);
    append_leaf(&lab, "planlog", "plan", "", Some("sha256:plan"), &data);
    let label = family::leaf_label("fam");
    append_leaf(&lab, "planlog", family::LEAF_KIND, &label, None, &data);
    plant_root(&lab, "planlog", 0, None);
    let p = confirm_refused(&lab, &root, &b);
    assert!(p.contains("The family records cannot be read: "), "{p}");
}

#[test]
fn a_plan_log_check_does_not_apply_test_log_leaf_indexes() {
    let lab = lab();
    let (root, b) = lone_member(&lab, |t| t.replace("test_log = false", "test_log = true"));
    let data = confirmation_data(&root);
    // Root fam's family lives in the test log: plan leaf 0, family leaf 1.
    let lock = lock_of_b(&b);
    append_leaf(&lab, "testlog", "plan", "", Some("sha256:plan"), &data);
    let label = family::leaf_label("fam");
    append_leaf(
        &lab,
        "testlog",
        family::LEAF_KIND,
        &label,
        Some(&lock.hash()),
        &data,
    );
    plant_root(&lab, "testlog", 0, Some(&lock));
    // An outsider's leaf 0 of the plan log holds b's data. Index 0 of the test log is the
    // root's plan leaf, which says nothing about the plan log.
    append_leaf(&lab, "planlog", "plan", "", Some("sha256:out"), &data);
    let p = confirm_refused(&lab, &root, &b);
    assert!(p.ends_with("The earlier entry is leaf 0 (plan)."), "{p}");
}

#[test]
fn a_locked_member_registers_only_as_locked_in_any_mode() {
    let lab = lab();
    let root = registered_family(&lab);
    let b = root.join("spec/b.toml");
    let locked = std::fs::read_to_string(&b).unwrap();
    // An exploratory plan takes no estimates, so it declares no deviation.
    let exploratory = ["[[pins.deviations]]", "from = ", "reason = ", "timing = "]
        .iter()
        .fold(locked, |t, k| {
            t.replace(&format!("\n{k}"), &format!("\n# {k}"))
        })
        .replace("mode = \"confirmatory\"", "mode = \"exploratory\"")
        .replace("label = \"confirmed\"", "label = \"signal\"");
    std::fs::write(&b, exploratory).unwrap();
    // `check` passes it: E-FAM-006 checks confirmatory plans.
    let (code, v) = check_json(&lab, &b);
    assert_eq!((code, errors(&v)), (0, vec![]), "{v}");
    let (code, out) = try_register(&lab, &b);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("E-FAM-006") && out.contains("Its spec hash is "),
        "{out}"
    );
    assert!(!lab.home.study_dir("b").exists());
    // b's slot stays reserved.
    let (_, v) = check_json(&lab, &outsider(&lab, "fresh data\n", |t| t));
    assert_eq!(v["plan_stream"]["reserved"], 2, "{v}");
}

#[test]
fn an_unreadable_plan_log_fails_a_member_closed() {
    let lab = lab();
    let root = registered_family(&lab);
    let leaves = lab.home.root.join("planlog/leaves.jsonl");
    let mut text = std::fs::read_to_string(&leaves).unwrap();
    text.push('{');
    std::fs::write(&leaves, text).unwrap();
    let (code, v) = check_json(&lab, &root.join("spec/b.toml"));
    let p = problem_at(&v, "E-FAM-006", "study.id");
    assert!(p.contains("cannot be read"), "{p}");
    assert_eq!(code, 1);
}

/// `text` with `follow_on` routes to `specs`.
fn following(text: String, specs: &[&str]) -> String {
    let list: Vec<String> = specs
        .iter()
        .map(|s| format!("{{ when = \"done\", spec = \"{s}\" }}"))
        .collect();
    text.replace(
        "follow_on = []",
        &format!("follow_on = [{}]", list.join(", ")),
    )
}

fn exploratory(text: String) -> String {
    text.replace("mode = \"confirmatory\"", "mode = \"exploratory\"")
}

/// The root `spec.toml`, pinned, that follows `a` and lists `members`; `a` is an exploratory
/// study that follows `b_routes`; `b` is a confirmatory study on the root's data, `edit`ed.
fn follow_case(
    lab: &Lab,
    members: &[&str],
    b_routes: &[&str],
    edit: impl Fn(String) -> String,
) -> Value {
    let root = family_root(lab);
    let a = exploratory(following(study("a"), b_routes));
    put(&root, "spec/a.toml", &a);
    put(&root, "spec/b.toml", &pinned(&root, &edit(study("b"))));
    let text = pinned(&root, &following(study("fam"), &["spec/a.toml"]));
    let spec = if members.is_empty() {
        put(&root, "spec.toml", &text)
    } else {
        root_spec(&root, &text, members)
    };
    check_json(lab, &spec).1
}

#[test]
fn a_follow_on_sharing_data_outside_the_family_warns() {
    let lab = lab();
    let v = follow_case(&lab, &[], &["spec/b.toml"], |t| t);
    let p = "scope.end_of_run.follow_on[0].spec";
    assert!(
        problem_at(&v, "W-FAM-002", p).contains("spec/b.toml"),
        "{v}"
    );
    assert!(!has(&v, "W-FAM-002", "scope.end_of_run.follow_on[1].spec"));
}

#[test]
fn a_member_follow_on_does_not_warn() {
    let lab = lab();
    let v = follow_case(&lab, &["spec/b.toml"], &["spec/b.toml"], |t| t);
    assert!(found(&v).iter().all(|(c, _)| c != "W-FAM-002"), "{v}");
}

#[test]
fn an_exploratory_follow_on_does_not_warn() {
    let lab = lab();
    let v = follow_case(&lab, &[], &["spec/b.toml"], exploratory);
    assert!(found(&v).iter().all(|(c, _)| c != "W-FAM-002"), "{v}");
}

#[test]
fn a_follow_on_cycle_terminates() {
    let lab = lab();
    let root = family_root(&lab);
    put(
        &root,
        "spec/a.toml",
        &exploratory(following(study("a"), &["spec/b.toml"])),
    );
    put(
        &root,
        "spec/b.toml",
        &pinned(&root, &following(study("b"), &["spec/a.toml"])),
    );
    let text = pinned(&root, &following(study("fam"), &["spec/a.toml"]));
    let (_, v) = check_json(&lab, &put(&root, "spec.toml", &text));
    assert!(
        has(&v, "W-FAM-002", "scope.end_of_run.follow_on[0].spec"),
        "{v}"
    );
}

/// Root `fam` with members `spec/b.toml` and `spec/c.toml`; `b` routes to `c`, and both hold
/// the root's confirmation data. Returns the root spec and `b`'s.
fn routed_family(lab: &Lab) -> (PathBuf, PathBuf) {
    let root = family_root(lab);
    let b = pinned(&root, &following(study("b"), &["spec/c.toml"]));
    let b = put(&root, "spec/b.toml", &b);
    put(&root, "spec/c.toml", &pinned(&root, &study("c")));
    let spec = root_spec(
        &root,
        &pinned(&root, &study("fam")),
        &["spec/b.toml", "spec/c.toml"],
    );
    (spec, b)
}

fn no_w_fam_002(v: &Value) {
    let d = v["diagnostics"].as_array().unwrap();
    assert!(d.iter().all(|d| d["code"] != "W-FAM-002"), "{v}");
}

#[test]
fn a_route_between_members_does_not_warn_at_the_root() {
    let lab = lab();
    let (spec, _) = routed_family(&lab);
    no_w_fam_002(&check_json(&lab, &spec).1);
}

#[test]
fn a_route_between_members_does_not_warn_at_a_registered_member() {
    let lab = lab();
    let (spec, b) = routed_family(&lab);
    register(&lab, &spec);
    no_w_fam_002(&check_json(&lab, &b).1);
}

#[test]
fn a_route_to_another_familys_member_still_warns() {
    let lab = lab();
    let (spec, b) = routed_family(&lab);
    register(&lab, &spec);
    let root = b.parent().unwrap().parent().unwrap();
    // `d` is outside the family and holds the same data.
    put(root, "spec/d.toml", &pinned(root, &study("d")));
    let text = std::fs::read_to_string(&b)
        .unwrap()
        .replace("spec/c.toml", "spec/d.toml");
    let v = check_json(
        &lab,
        &put(
            root,
            "spec/e.toml",
            &text.replace("id = \"b\"", "id = \"e\""),
        ),
    )
    .1;
    assert!(
        has(&v, "W-FAM-002", "scope.end_of_run.follow_on[0].spec"),
        "{v}"
    );
}
