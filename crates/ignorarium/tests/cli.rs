mod common;

use assert_cmd::Command;
use ignorarium::cli::check::{self, HomeRecords};
use ignorarium::home::Home;
use ignorarium::keys;
use ignorarium::rules::BoxRecords;
use ignorarium::stream::{self, LogName};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const TEMPLATE: &str = "../../templates/numeric-ablation.toml";

/// The binary with its own box home; the test process env stays untouched.
fn bin(home: &Path) -> Command {
    let mut c = Command::cargo_bin("ignorarium").unwrap();
    c.env("IGNORARIUM_HOME", home);
    c
}

fn stdout(a: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(a.get_output().stdout.clone()).unwrap()
}

fn stderr(a: &assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&a.get_output().stderr).into_owned()
}

fn json_out(a: &assert_cmd::assert::Assert) -> Value {
    serde_json::from_str(&stdout(a)).unwrap()
}

/// A box home with `max_level = 1`, and a spec file made from the template.
fn setup(edit: impl Fn(String) -> String) -> (TempDir, PathBuf, PathBuf) {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(home.join("machine.toml"), "max_level = 1\n").unwrap();
    let spec = dir.path().join("spec.toml");
    let text = std::fs::read_to_string(TEMPLATE).unwrap();
    std::fs::write(&spec, edit(text)).unwrap();
    (dir, home, spec)
}

fn no_unit(t: String) -> String {
    t.replace("unit = \"ratio\"", "unit = \"\"")
}

/// Drops the power line so the default fills it.
fn no_power(t: String) -> String {
    t.replace("power = 0.8\n", "")
}

#[test]
fn guide_json_lists_every_step_with_its_output() {
    let dir = TempDir::new().unwrap();
    let a = bin(dir.path()).args(["guide", "--json"]).assert().success();
    let steps = json_out(&a);
    let steps = steps.as_array().unwrap();
    let ids: Vec<&str> = steps.iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        [
            "brief", "template", "fill", "payload", "import", "check", "review", "register", "run",
            "status", "report"
        ]
    );
    for s in steps {
        for key in ["title", "does", "produces", "command"] {
            assert!(!s[key].as_str().unwrap().is_empty(), "{s}");
        }
    }
    let produces = |id: &str| {
        let s = steps.iter().find(|s| s["id"] == id).unwrap();
        s["produces"].as_str().unwrap().to_string()
    };
    assert!(produces("payload").contains("payload/run"));
    for part in ["payload", "store"] {
        assert!(produces("register").contains(part), "{part}");
    }
    let report = steps.iter().find(|s| s["id"] == "report").unwrap();
    assert!(
        report["does"]
            .as_str()
            .unwrap()
            .contains("not available yet")
    );
}

#[test]
fn guide_prints_steps_in_order() {
    let dir = TempDir::new().unwrap();
    let a = bin(dir.path()).arg("guide").assert().success();
    let out = stdout(&a);
    let at = |id: &str| {
        out.find(&format!(". {id}: "))
            .unwrap_or_else(|| panic!("{id}"))
    };
    assert!(at("brief") < at("template") && at("template") < at("report"));
    assert!(at("fill") < at("payload") && at("payload") < at("import"));
    assert!(at("import") < at("check"));
    assert!(out.contains("ignorarium check"));
}

#[test]
fn check_passes_the_template_and_prints_the_estimate() {
    let (_d, home, spec) = setup(no_power);
    let a = bin(&home).arg("check").arg(&spec).assert().code(0);
    let out = stdout(&a);
    for want in [
        "exact: 1 rows",
        "arm-a: 1 rows",
        "rows: 2",
        "cpu hours: 0.03",
        "wall hours: 0.03",
        "money: $0.00",
        "long job: no",
        "label: signal",
        "design.power = 0.8",
    ] {
        assert!(out.contains(want), "missing {want:?} in:\n{out}");
    }
    // The hash is over the author's file, not the defaulted value.
    let hash = ignorarium::spec::load(&spec).unwrap().hash;
    assert!(out.contains(&format!("spec: {hash}")), "{out}");
}

#[test]
fn check_fails_bad_fixture_with_code_and_instruction() {
    let (_d, home, spec) = setup(no_unit);
    let a = bin(&home).arg("check").arg(&spec).assert().code(1);
    let out = stdout(&a);
    assert!(out.contains("error[E-OUT-001]"), "{out}");
    assert!(out.contains("fix:"));
    assert!(out.contains("docs/rules/E-OUT-001.md"));
    assert!(!out.contains("label:"), "no estimate when the plan fails");
}

#[test]
fn check_json_is_machine_readable() {
    let (_d, home, spec) = setup(no_unit);
    let a = bin(&home)
        .arg("check")
        .arg(&spec)
        .arg("--json")
        .assert()
        .code(1);
    let v = json_out(&a);
    let d = &v["diagnostics"][0];
    assert_eq!(d["code"], "E-OUT-001");
    assert_eq!(d["path"], "outcomes[0].unit");
    assert!(!d["instruction"].as_str().unwrap().is_empty());
    assert!(v["estimate"].is_null());
    assert_eq!(v["hash"], ignorarium::spec::load(&spec).unwrap().hash);
}

#[test]
fn check_json_passing_plan_has_estimate_and_defaults() {
    let (_d, home, spec) = setup(no_power);
    let a = bin(&home)
        .arg("check")
        .arg(&spec)
        .arg("--json")
        .assert()
        .code(0);
    let v = json_out(&a);
    let codes: Vec<&str> = v["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["W-LIC-001", "W-LIC-001", "W-HSH-001", "W-HSH-001"]);
    assert_eq!(v["estimate"]["rows"], 2);
    assert_eq!(v["estimate"]["label"], "signal");
    assert_eq!(v["estimate"]["long_job"], false);
    assert!(
        v["defaults"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["path"] == "design.power" && d["value"] == 0.8)
    );
}

#[test]
fn check_uses_the_machine_level_from_the_box() {
    let (_d, home, spec) = setup(|t| t);
    std::fs::remove_file(home.join("machine.toml")).unwrap();
    let a = bin(&home).arg("check").arg(&spec).assert().code(1);
    assert!(stdout(&a).contains("error[E-ISO-001]"));
}

#[test]
fn check_io_and_usage_errors_exit_2() {
    let dir = TempDir::new().unwrap();
    bin(dir.path())
        .args(["check", "missing.toml"])
        .assert()
        .code(2);
    bin(dir.path()).arg("check").assert().code(2);
    bin(dir.path()).arg("nonsense").assert().code(2);
}

#[test]
fn check_refuses_a_spec_that_is_not_toml() {
    let (_d, home, spec) = setup(|_| "[study\n".into());
    let a = bin(&home).arg("check").arg(&spec).assert().code(1);
    assert!(stderr(&a).contains("TOML error at 1:"), "{}", stderr(&a));
}

#[test]
fn review_has_all_sections_and_the_hash() {
    let (_d, home, spec) = setup(|t| t);
    let a = bin(&home).arg("review").arg(&spec).assert().code(0);
    let out = stdout(&a);
    let mut at = 0;
    for h in [
        "## Plan",
        "## Conditions",
        "## Estimate",
        "## Claim label",
        "## Defaults",
        "## Warnings",
        "## Inputs and licences",
    ] {
        let i = out[at..]
            .find(h)
            .unwrap_or_else(|| panic!("{h} in order:\n{out}"));
        at += i + h.len();
    }
    let hash = ignorarium::spec::load(&spec).unwrap().hash;
    assert!(out.contains(&format!("Spec hash: {hash}")));
    assert!(out.contains("- exact: 1 rows"));
    assert!(out.contains("data/example.txt"));
}

#[test]
fn review_lists_each_claim_with_its_strata_and_statistic() {
    let second = "[[question.hypotheses]]
id = \"P1\"
statement = \"The spread per text.\"
rival = \"none\"
outcome = \"ppl-ratio\"
contrast = { arm-a = 1.0, exact = 0.5 }

[[outcomes]]";
    let (_d, home, spec) = setup(|t| {
        common::with_claim(t)
            .replacen("statement = \"\"", "statement = \"The arm lowers it.\"", 1)
            .replacen("rival = \"\"", "rival = \"Chance.\"", 1)
            .replace("strata = []", "strata = [\"fiction\"]")
            .replacen("\n[[outcomes]]", &format!("\n{second}"), 1)
    });
    // P1's contrast names the reference: review still prints the plan as written.
    let a = bin(&home).arg("review").arg(&spec).assert().code(1);
    let out = stdout(&a);
    let lines = [
        "- H1: The arm lowers it. (rival: Chance.); outcome: ppl-ratio; contrast: arm-a 1.0; \
         strata: fiction; claim: below 0.0; statistic: quantile 0.5; alpha share: 1.0\n",
        "- P1: The spread per text. (rival: none); outcome: ppl-ratio; contrast: arm-a 1.0, \
         exact 0.5; strata: pooled; claim: none (descriptive)\n",
    ];
    let hypotheses = &out[out.find("Hypotheses:").expect(&out)..out.find("Outcomes:").unwrap()];
    assert_eq!(
        hypotheses,
        format!("Hypotheses:\n\n{}{}\n", lines[0], lines[1])
    );
    assert!(
        out.contains("E-HYP-001 at `question.hypotheses[1].contrast.exact`"),
        "{out}"
    );
}

#[test]
fn review_of_a_failing_plan_lists_the_errors() {
    let (_d, home, spec) = setup(no_unit);
    let a = bin(&home).arg("review").arg(&spec).assert().code(1);
    let out = stdout(&a);
    let warnings = &out[out.find("## Warnings").unwrap()..];
    let (errors, rest) = warnings.split_once("### Warnings").expect(&out);
    assert!(errors.contains("### Errors"), "{out}");
    assert!(errors.contains("E-OUT-001") && !errors.contains("W-HSH-001"));
    assert!(rest.contains("W-HSH-001") && !rest.contains("E-OUT-001"));
    assert!(out.contains("## Estimate"));
    assert!(out.contains("not available"));
}

#[test]
fn new_copies_a_template_with_the_given_id() {
    let dir = TempDir::new().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let home = dir.path().join("home");
    bin(&home)
        .current_dir(&work)
        .args(["new", "--template", "numeric-ablation", "--id", "my-run-2"])
        .assert()
        .code(0);
    let path = work.join("studies/my-run-2/spec.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("id = \"my-run-2\"                 # ASCII kebab-case"));
    assert_eq!(text.matches("my-study").count(), 0);
    // The folder now exists: a second call refuses and leaves the file alone.
    bin(&home)
        .current_dir(&work)
        .args(["new", "--template", "numeric-ablation", "--id", "my-run-2"])
        .assert()
        .code(1);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn new_rejects_unknown_template_and_bad_id() {
    let dir = TempDir::new().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let home = dir.path().join("home");
    let a = bin(&home)
        .current_dir(&work)
        .args(["new", "--template", "nope", "--id", "a"])
        .assert()
        .code(2);
    let err = stderr(&a);
    assert!(err.contains("numeric-ablation") && err.contains("planted-spawn"));
    for bad in ["My Study", "../x", "a--b", "é"] {
        bin(&home)
            .current_dir(&work)
            .args(["new", "--template", "numeric-ablation", "--id", bad])
            .assert()
            .code(2);
    }
    assert!(!work.join("studies").exists());
}

#[test]
fn each_verb_emits_one_event() {
    let (_d, home, spec) = setup(|t| t);
    bin(&home).arg("check").arg(&spec).assert().code(0);
    let log = std::fs::read_to_string(home.join("events.jsonl")).unwrap();
    let lines: Vec<Value> = log
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["name"], "check");
    assert_eq!(lines[0]["attrs"]["exit"], 0);
    let hash = ignorarium::spec::load(&spec).unwrap().hash;
    assert_eq!(lines[0]["attrs"]["spec_hash"], hash.as_str());
    assert_eq!(lines[0]["trace_id"].as_str().unwrap().len(), 32);
}

#[test]
fn an_unreadable_plan_log_surfaces_as_a_diagnostic() {
    let (dir, home, _) = setup(|t| t);
    let study = dir.path().join("study");
    std::fs::create_dir_all(study.join("data")).unwrap();
    std::fs::write(study.join("data/example.txt"), "hello").unwrap();
    // A torn last line: the log cannot be read.
    std::fs::create_dir_all(home.join("planlog")).unwrap();
    std::fs::write(home.join("planlog/leaves.jsonl"), "{").unwrap();
    let sha = ignorarium::canon::sha256_hex(b"hello");
    let text = std::fs::read_to_string(TEMPLATE)
        .unwrap()
        .replace("mode = \"exploratory\"", "mode = \"confirmatory\"")
        .replace("label = \"signal\"", "label = \"confirmed\"")
        .replace("sha256 = \"\"", &format!("sha256 = \"{sha}\""))
        .replace("role = \"calibration\"", "role = \"confirmation\"");
    std::fs::write(study.join("spec.toml"), text).unwrap();
    let a = bin(&home)
        .arg("check")
        .arg(study.join("spec.toml"))
        .assert()
        .code(1);
    let out = stdout(&a);
    assert!(out.contains("E-CNF-005"), "{out}");
    assert!(out.contains("cannot be read"), "{out}");
}

#[cfg(unix)]
#[test]
fn an_empty_home_variable_falls_back_to_the_user_home() {
    let (dir, _, spec) = setup(|t| t);
    let user = dir.path().join("user");
    std::fs::create_dir(&user).unwrap();
    Command::cargo_bin("ignorarium")
        .unwrap()
        .env("IGNORARIUM_HOME", "")
        .env("HOME", &user)
        .arg("check")
        .arg(&spec)
        .output()
        .unwrap();
    assert!(user.join(".ignorarium").is_dir());
}

#[test]
fn new_reports_a_folder_it_cannot_create_apart_from_one_that_exists() {
    let dir = TempDir::new().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    // 300 characters pass the id rule but exceed a file name limit.
    let a = bin(&dir.path().join("home"))
        .current_dir(&work)
        .args([
            "new",
            "--template",
            "numeric-ablation",
            "--id",
            &"a".repeat(300),
        ])
        .assert()
        .code(2);
    let err = stderr(&a);
    assert!(err.contains("cannot create the study folder"), "{err}");
    assert!(!err.contains("already exists"), "{err}");
}

fn token() -> String {
    format!("AKIA{}", "ABCDEFGHIJKLMNOP")
}

#[test]
fn review_withholds_a_flagged_value() {
    let t = token();
    let (_d, home, spec) =
        setup(|s| s.replace("population = \"\"", &format!("population = \"key {t}\"")));
    let a = bin(&home).arg("review").arg(&spec).assert().code(1);
    let out = stdout(&a);
    assert!(!out.contains(&t), "{out}");
    assert!(out.contains("Population: (withheld: E-SEC-001)"), "{out}");
}

#[test]
fn an_unknown_key_that_looks_like_a_token_is_never_printed() {
    let t = token();
    let (_d, home, spec) = setup(|s| {
        s.replace(
            "test_log = false",
            &format!("test_log = false\n\"{t}\" = 1"),
        )
    });
    for args in [vec!["check"], vec!["check", "--json"], vec!["review"]] {
        let mut c = bin(&home);
        c.args(&args).arg(&spec);
        let a = c.assert().code(1);
        let out = stdout(&a);
        assert!(!out.contains(&t), "{args:?}: {out}");
        assert!(!stderr(&a).contains(&t));
        assert!(out.contains("study.<key>"), "{args:?}: {out}");
    }
}

#[test]
fn new_changes_only_the_study_id_and_the_result_checks() {
    for name in ["numeric-ablation", "planted-spawn"] {
        let dir = TempDir::new().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir(&work).unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir(&home).unwrap();
        std::fs::write(home.join("machine.toml"), "max_level = 1\n").unwrap();
        bin(&home)
            .current_dir(&work)
            .args(["new", "--template", name, "--id", "zz-1"])
            .assert()
            .code(0);
        let spec = work.join("studies/zz-1/spec.toml");
        let made = std::fs::read_to_string(&spec).unwrap();
        let orig = std::fs::read_to_string(format!("../../templates/{name}.toml")).unwrap();
        let diff: Vec<(&str, &str)> = orig
            .lines()
            .zip(made.lines())
            .filter(|(a, b)| a != b)
            .collect();
        assert_eq!(orig.lines().count(), made.lines().count());
        assert_eq!(diff.len(), 1, "{diff:?}");
        assert!(diff[0].1.starts_with("id = \"zz-1\""));
        assert!(made.contains("id = \"H1\"") && made.contains("id = \"exact\""));
        bin(&home).arg("check").arg(&spec).assert().code(0);
    }
}

#[test]
fn check_resolves_inputs_from_a_bare_filename() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(home.join("machine.toml"), "max_level = 1\n").unwrap();
    std::fs::create_dir(dir.path().join("data")).unwrap();
    std::fs::write(dir.path().join("data/example.txt"), "hello").unwrap();
    let text = std::fs::read_to_string(TEMPLATE).unwrap();
    let write = |sha: &str| {
        std::fs::write(
            dir.path().join("spec.toml"),
            text.replacen("sha256 = \"\"", &format!("sha256 = \"{sha}\""), 1),
        )
        .unwrap();
    };
    write(&ignorarium::canon::sha256_hex(b"hello"));
    let a = bin(&home)
        .current_dir(dir.path())
        .args(["check", "spec.toml"])
        .assert()
        .code(0);
    assert!(!stdout(&a).contains("E-HSH-001"));
    // A wrong pin fails with the same call: the file was found and hashed.
    write(&"0".repeat(64));
    let a = bin(&home)
        .current_dir(dir.path())
        .args(["check", "spec.toml"])
        .assert()
        .code(1);
    assert!(stdout(&a).contains("E-HSH-001"));
}

/// A study root with `ignorarium.toml`, the input at `root/data/example.txt`, and a
/// spec in `root/spec/` made from the template.
fn rooted(edit: impl Fn(String) -> String) -> (TempDir, PathBuf, PathBuf) {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(
        home.join("machine.toml"),
        "max_level = 1
",
    )
    .unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir_all(root.join("spec")).unwrap();
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("ignorarium.toml"), "").unwrap();
    std::fs::write(root.join("data/example.txt"), "hello").unwrap();
    let text = std::fs::read_to_string(TEMPLATE).unwrap();
    let spec = root.join("spec/s.toml");
    std::fs::write(&spec, edit(text)).unwrap();
    (dir, home, spec)
}

/// The first input pinned to the bytes `hello`.
fn pinned(t: String) -> String {
    t.replacen(
        "sha256 = \"\"",
        &format!("sha256 = \"{}\"", ignorarium::canon::sha256_hex(b"hello")),
        1,
    )
}

#[test]
fn a_spec_in_spec_reads_inputs_from_the_root() {
    let (_d, home, spec) = rooted(pinned);
    let a = bin(&home).arg("check").arg(&spec).assert().code(0);
    assert!(!stdout(&a).contains("E-HSH-001"));
    // A wrong pin fails with the same call: the file was found and hashed.
    std::fs::write(
        &spec,
        pinned(std::fs::read_to_string(TEMPLATE).unwrap())
            .replace(&ignorarium::canon::sha256_hex(b"hello"), &"0".repeat(64)),
    )
    .unwrap();
    let a = bin(&home).arg("check").arg(&spec).assert().code(1);
    assert!(stdout(&a).contains("E-HSH-001"));
}

#[test]
fn follow_on_specs_resolve_from_the_root() {
    let follow = |t: String| {
        t.replace(
            "follow_on = []",
            "follow_on = [{ when = \"done\", spec = \"spec/next.toml\" }]",
        )
    };
    let (_d, home, spec) = rooted(follow);
    let root = spec.parent().unwrap().parent().unwrap();
    let a = bin(&home).arg("check").arg(&spec).assert().code(1);
    assert!(stdout(&a).contains("E-END-002"), "{}", stdout(&a));
    std::fs::write(
        root.join("spec/next.toml"),
        "x = 1
",
    )
    .unwrap();
    let a = bin(&home).arg("check").arg(&spec).assert().code(0);
    assert!(!stdout(&a).contains("E-END-002"));
}

/// A temp folder that holds `.git`, so the root search stops inside it and a stray
/// marker in `/tmp` or above cannot reach the test.
fn repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    dir
}

fn mark(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("ignorarium.toml"), "").unwrap();
}

#[test]
fn no_marker_keeps_the_spec_folder() {
    let dir = repo();
    let spec = dir.path().join("a/b/s.toml");
    assert_eq!(check::study_root(&spec), dir.path().join("a/b"));
}

#[test]
fn a_relative_spec_without_a_marker_roots_at_the_current_folder() {
    let dir = repo();
    std::fs::write(dir.path().join("s.toml"), "").unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let a = bin(&home)
        .current_dir(dir.path())
        .args(["review", "s.toml"])
        .assert();
    assert!(
        stdout(&a).contains(
            "Study root: .
"
        ),
        "{}",
        stdout(&a)
    );
}

#[test]
fn the_nearest_marker_above_the_spec_wins() {
    let dir = repo();
    let inner = dir.path().join("outer/inner");
    std::fs::create_dir_all(inner.join("deep")).unwrap();
    mark(&dir.path().join("outer"));
    let spec = inner.join("deep/s.toml");
    assert_eq!(check::study_root(&spec), dir.path().join("outer"));
    mark(&inner);
    assert_eq!(check::study_root(&spec), inner);
    mark(&inner.join("deep"));
    assert_eq!(check::study_root(&spec), inner.join("deep"));
}

#[test]
fn a_marker_that_is_a_folder_or_a_link_is_not_a_marker() {
    let dir = repo();
    let folder = dir.path().join("f");
    std::fs::create_dir_all(folder.join("spec/ignorarium.toml")).unwrap();
    assert_eq!(
        check::study_root(&folder.join("spec/s.toml")),
        folder.join("spec")
    );
    #[cfg(unix)]
    {
        let linked = dir.path().join("l");
        std::fs::create_dir_all(linked.join("spec")).unwrap();
        std::fs::write(linked.join("real.toml"), "").unwrap();
        std::os::unix::fs::symlink(linked.join("real.toml"), linked.join("ignorarium.toml"))
            .unwrap();
        assert_eq!(
            check::study_root(&linked.join("spec/s.toml")),
            linked.join("spec")
        );
    }
}

#[test]
fn the_search_stops_at_the_repo_root_but_checks_it() {
    let dir = TempDir::new().unwrap();
    mark(dir.path());
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(repo.join("sub/spec")).unwrap();
    let spec = repo.join("sub/spec/s.toml");
    // The marker above the repo is out of reach.
    assert_eq!(check::study_root(&spec), repo.join("sub/spec"));
    // A marker in the repo root itself counts.
    mark(&repo);
    assert_eq!(check::study_root(&spec), repo);
}

#[test]
fn the_search_never_uses_a_marker_in_the_home_folder() {
    let dir = repo();
    let home = dir.path().join("home");
    mark(&home);
    let spec = home.join("study/s.toml");
    assert_eq!(check::study_root_in(&spec, Some(&home)), home.join("study"));
    assert_eq!(check::study_root_in(&spec, None), home);
}

#[test]
fn dotdot_in_the_spec_argument_is_folded_before_the_walk() {
    let dir = repo();
    mark(&dir.path().join("a"));
    std::fs::create_dir_all(dir.path().join("b")).unwrap();
    let spec = dir.path().join("a/../b/s.toml");
    assert_eq!(check::study_root(&spec), dir.path().join("b"));
}

#[test]
fn review_shows_the_root_relative_to_the_current_folder_when_inside_it() {
    let (d, home, spec) = rooted(|t| t);
    let root = spec.parent().unwrap().parent().unwrap();
    let a = bin(&home).arg("review").arg(&spec).assert();
    assert!(stdout(&a).contains(&format!(
        "Study root: {}
",
        root.display()
    )));
    let a = bin(&home)
        .current_dir(d.path())
        .args(["review", "root/spec/s.toml"])
        .assert();
    assert!(
        stdout(&a).contains(
            "Study root: root
"
        ),
        "{}",
        stdout(&a)
    );
}

#[test]
fn dotdot_is_still_refused_under_a_root() {
    let (_d, home, spec) = rooted(|t| t.replace("data/example.txt", "../x"));
    let a = bin(&home).arg("check").arg(&spec).assert().code(1);
    assert!(stdout(&a).contains("E-PTH-001"), "{}", stdout(&a));
}

#[test]
fn check_json_reports_a_load_failure_as_one_object() {
    let (_d, home, spec) = setup(|_| "[study\n".into());
    let a = bin(&home)
        .arg("check")
        .arg(&spec)
        .arg("--json")
        .assert()
        .code(1);
    let v = json_out(&a);
    assert_eq!(v["diagnostics"].as_array().unwrap().len(), 0);
    assert!(v["error"].as_str().unwrap().contains("TOML error at 1:"));
    assert!(v["hash"].is_null());
    assert!(v.get("estimate").is_none());
    let a = bin(&home)
        .args(["check", "--json", "missing.toml"])
        .assert()
        .code(2);
    assert!(
        json_out(&a)["error"]
            .as_str()
            .unwrap()
            .contains("missing.toml")
    );
}

#[test]
fn guide_wording_is_public() {
    let dir = TempDir::new().unwrap();
    let a = bin(dir.path()).arg("guide").assert().success();
    let out = stdout(&a);
    assert!(
        !regex::Regex::new(r"Plan \d").unwrap().is_match(&out),
        "{out}"
    );
    assert_eq!(out.matches("not available yet").count(), 3);
}

#[cfg(target_os = "linux")]
#[test]
fn a_stdout_write_error_other_than_a_closed_pipe_exits_2() {
    let dir = TempDir::new().unwrap();
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let st = std::process::Command::new(env!("CARGO_BIN_EXE_ignorarium"))
        .env("IGNORARIUM_HOME", dir.path())
        .arg("guide")
        .stdout(full)
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert_eq!(st.code(), Some(2));
}

/// A hung-up terminal fails every write, as `/dev/full` does here.
#[cfg(target_os = "linux")]
#[test]
fn an_error_that_cannot_reach_stderr_keeps_its_exit_code_and_its_event() {
    let dir = TempDir::new().unwrap();
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let st = std::process::Command::new(env!("CARGO_BIN_EXE_ignorarium"))
        .env("IGNORARIUM_HOME", dir.path())
        .args(["status", "nope"])
        .stdout(std::process::Stdio::null())
        .stderr(full)
        .status()
        .unwrap();
    assert_eq!(st.code(), Some(1));
    let ev = events(dir.path());
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0]["name"], "status");
    assert_eq!(ev[0]["attrs"]["exit"], 1);
}

fn events(home: &Path) -> Vec<Value> {
    std::fs::read_to_string(home.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn register_refuses_without_a_terminal() {
    let (_d, home, spec) = setup(|t| t);
    let a = bin(&home)
        .arg("register")
        .arg(&spec)
        .write_stdin("00000000\n")
        .assert()
        .code(1);
    let err = stderr(&a);
    assert!(err.contains("E-REG-001"), "{err}");
    assert!(err.contains("docs/rules/E-REG-001.md"), "{err}");
    assert!(!home.join("studies/my-study").exists());
    let ev = events(&home);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0]["name"], "register");
    assert_eq!(ev[0]["attrs"]["exit"], 1);
}

/// A confirmatory spec in `dir` whose selection input is `data.txt`.
fn reuse_spec(dir: &Path, bytes: &[u8]) -> PathBuf {
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("data/example.txt"), bytes).unwrap();
    let text = std::fs::read_to_string(TEMPLATE)
        .unwrap()
        .replace("mode = \"exploratory\"", "mode = \"confirmatory\"")
        .replace("label = \"signal\"", "label = \"confirmed\"")
        .replace(
            "sha256 = \"\"",
            &format!("sha256 = \"{}\"", ignorarium::canon::sha256_hex(bytes)),
        )
        .replace("role = \"calibration\"", "role = \"selection\"");
    let spec = dir.join("spec.toml");
    std::fs::write(&spec, text).unwrap();
    spec
}

#[test]
fn import_history_then_confirmatory_reuse_fails_check() {
    let (d, home, _) = setup(|t| t);
    let old = d.path().join("old.txt");
    std::fs::write(&old, "Some  Earlier TEXT").unwrap();
    let a = bin(&home)
        .args(["import-history", "--phase", "pilot"])
        .arg(&old)
        .assert()
        .code(0);
    assert_eq!(stdout(&a), "leaf 0\n");
    // Same bytes, and the same text with other case and spacing.
    for bytes in [&b"Some  Earlier TEXT"[..], b"some earlier\ntext"] {
        let spec = reuse_spec(&d.path().join("study"), bytes);
        let a = bin(&home).arg("check").arg(&spec).assert().code(1);
        let out = stdout(&a);
        assert!(out.contains("error[E-CNF-005]"), "{out}");
        assert!(out.contains("leaf 0 (legacy)"), "{out}");
    }
    let ev = events(&home);
    assert_eq!(ev[0]["name"], "import-history");
    assert_eq!(ev[0]["attrs"]["exit"], 0);
    assert_eq!(ev[0]["attrs"]["leaf"], 0);
}

fn leaf_lines(home: &Path, log: &str) -> Vec<Value> {
    std::fs::read_to_string(home.join(log).join("leaves.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn import_history_records_one_leaf_with_every_files_hashes() {
    let (d, home, _) = setup(|t| t);
    let (a, b) = (d.path().join("a.txt"), d.path().join("b.bin"));
    std::fs::write(&a, "Hello  World").unwrap();
    std::fs::write(&b, [0xff, 0xfe, 0x00]).unwrap();
    bin(&home)
        .args(["import-history", "--phase", "pilot"])
        .args([&a, &b])
        .assert()
        .code(0);
    let leaves = leaf_lines(&home, "planlog");
    assert_eq!(leaves.len(), 1);
    let l = &leaves[0];
    assert_eq!(l["kind"], "legacy");
    assert_eq!(l["label"], "pilot");
    assert!(l["commitment"].is_null() && l["family"].is_null() && l["mode"].is_null());
    let sha = ignorarium::canon::sha256_hex;
    assert_eq!(
        l["data_hashes"],
        serde_json::json!([
            [sha(b"Hello  World"), sha(b"hello world")],
            [sha(&[0xff, 0xfe, 0x00]), ""]
        ])
    );
    // A second call adds a second leaf.
    let a2 = bin(&home)
        .args(["import-history", "--phase", "later"])
        .arg(&a)
        .assert()
        .code(0);
    assert_eq!(stdout(&a2), "leaf 1\n");
}

#[test]
fn import_history_refuses_a_bad_file_before_appending() {
    let (d, home, _) = setup(|t| t);
    let good = d.path().join("good.txt");
    std::fs::write(&good, "x").unwrap();
    for bad in [d.path().join("missing.txt"), d.path().to_path_buf()] {
        let a = bin(&home)
            .args(["import-history", "--phase", "p"])
            .arg(&good)
            .arg(&bad)
            .assert()
            .code(2);
        assert!(stderr(&a).contains(&bad.display().to_string()));
    }
    assert!(leaf_lines(&home, "planlog").is_empty());
}

#[test]
fn import_history_refuses_a_phase_in_the_form_of_a_studys_own_label() {
    let (d, home, _) = setup(|t| t);
    let file = d.path().join("rows.csv");
    std::fs::write(&file, "x").unwrap();
    let a = bin(&home)
        .args(["import-history", "--phase", "study:pilot"])
        .arg(&file)
        .assert()
        .code(2);
    assert!(stderr(&a).contains("study:"), "{}", stderr(&a));
    assert!(leaf_lines(&home, "planlog").is_empty());
    // A colon elsewhere is a plain phase name.
    bin(&home)
        .args(["import-history", "--phase", "pilot:study:1"])
        .arg(&file)
        .assert()
        .code(0);
    let help = bin(&home)
        .args(["import-history", "--help"])
        .assert()
        .success();
    assert!(stdout(&help).contains("Must not start with `study:`"));
}

/// Give the study root of `spec` a payload folder, as `register` needs.
fn give_payload(spec: &Path) {
    let dir = spec.parent().unwrap().join("payload");
    std::fs::create_dir_all(&dir).unwrap();
    common::write_exec(&dir.join("run"), common::PAYLOAD);
}

/// Register the template through the library, then return the box. The box's payload copy
/// is made writable again, so the temp folder can be removed.
fn registered() -> (TempDir, PathBuf, PathBuf) {
    let (d, home, spec) = setup(|t| t);
    give_payload(&spec);
    let h = ignorarium::home::Home::open_at(&home).unwrap();
    let typed = format!("{}\n", &ignorarium::spec::load(&spec).unwrap().hash[7..15]);
    let mut out = Vec::new();
    ignorarium::cli::register::run(
        &spec,
        &h,
        &mut std::io::Cursor::new(typed.into_bytes()),
        &mut out,
        true,
    )
    .unwrap();
    common::unseal(&home);
    (d, home, spec)
}

#[test]
fn verify_passes_after_a_register_and_counts_leaves() {
    let (_d, home, _) = registered();
    let a = bin(&home)
        .args(["verify", "--study", "my-study"])
        .assert()
        .code(0);
    let out = stdout(&a);
    assert!(out.contains("planlog: 1 signed, 1 leaves"), "{out}");
    assert!(out.contains("testlog: 0 signed, 0 leaves"), "{out}");
    assert!(out.contains("study my-study: 1 records"), "{out}");
    assert!(!out.contains("warning"), "{out}");
    let ev = events(&home);
    assert_eq!(ev.last().unwrap()["name"], "verify");
    assert_eq!(ev.last().unwrap()["attrs"]["exit"], 0);
}

#[test]
fn verify_warns_about_unsigned_leaves() {
    let (_d, home, _) = registered();
    let p = home.join("planlog/leaves.jsonl");
    let mut text = std::fs::read_to_string(&p).unwrap();
    let again = text.clone();
    text.push_str(&again);
    std::fs::write(&p, text).unwrap();
    let a = bin(&home).arg("verify").assert().code(0);
    let out = stdout(&a);
    assert!(out.contains("planlog: 1 signed, 2 leaves"), "{out}");
    assert!(
        out.contains("warning: planlog has 1 unsigned leaves"),
        "{out}"
    );
}

#[test]
fn verify_fails_on_a_tampered_leaf_naming_the_log() {
    let (_d, home, _) = registered();
    let p = home.join("planlog/leaves.jsonl");
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(text.contains("signal"));
    std::fs::write(&p, text.replace("signal", "signaL")).unwrap();
    let a = bin(&home).arg("verify").assert().code(1);
    assert!(stderr(&a).contains("planlog"), "{}", stderr(&a));
}

#[test]
fn verify_study_fails_on_a_tampered_record() {
    let (_d, home, _) = registered();
    bin(&home).arg("verify").assert().code(0);
    let p = home.join("studies/my-study/records.jsonl");
    let text = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, text.replace("register", "regista")).unwrap();
    // Without --study the record chain is not read.
    bin(&home).arg("verify").assert().code(0);
    let a = bin(&home)
        .args(["verify", "--study", "my-study"])
        .assert()
        .code(1);
    assert!(stderr(&a).contains("my-study"), "{}", stderr(&a));
}

#[test]
fn verify_study_fails_on_a_folder_with_no_register_record() {
    let (_d, home, _) = registered();
    let rp = home.join("studies/my-study/records.jsonl");
    std::fs::write(&rp, b"").unwrap();
    let a = bin(&home)
        .args(["verify", "--study", "my-study"])
        .assert()
        .code(1);
    assert!(stderr(&a).contains("no register record"), "{}", stderr(&a));
    // A folder with no records file at all is the same.
    std::fs::remove_file(&rp).unwrap();
    bin(&home)
        .args(["verify", "--study", "my-study"])
        .assert()
        .code(1);
}

#[test]
fn verify_study_needs_a_known_id() {
    let (_d, home, _) = registered();
    for id in ["nope", "../x", ".."] {
        bin(&home).args(["verify", "--study", id]).assert().code(1);
    }
}

#[test]
fn review_never_prints_an_unsafe_key_or_a_flagged_array_value() {
    let (k, v, c) = (
        token(),
        format!("ghp_{}", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8"),
        token().replace("AKIA", "AKIB"),
    );
    let (_d, home, spec) = setup(|s| {
        s.replace(
            "test_log = false",
            &format!(
                "test_log = false
\"UP_{k}\" = \"{v}\""
            ),
        )
        .replace("creators = []", &format!("creators = [\"owner {c}\"]"))
    });
    let a = bin(&home).arg("review").arg(&spec).assert().code(1);
    let out = stdout(&a);
    for secret in [&k, &v, &c] {
        assert!(!out.contains(secret.as_str()), "{out}");
    }
}

#[test]
fn register_checks_for_a_terminal_before_opening_the_home() {
    let d = TempDir::new().unwrap();
    let not_a_dir = d.path().join("file");
    std::fs::write(&not_a_dir, "x").unwrap();
    let a = bin(&not_a_dir)
        .arg("register")
        .arg(d.path().join("spec.toml"))
        .write_stdin("")
        .assert()
        .code(1);
    assert!(stderr(&a).contains("E-REG-001"), "{}", stderr(&a));
}

#[cfg(unix)]
#[test]
fn import_history_refuses_a_symlink() {
    let (d, home, _) = setup(|t| t);
    let real = d.path().join("real.txt");
    std::fs::write(&real, "x").unwrap();
    let link = d.path().join("link.txt");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    bin(&home)
        .args(["import-history", "--phase", "p"])
        .arg(&link)
        .assert()
        .code(2);
    assert!(leaf_lines(&home, "planlog").is_empty());
}

#[test]
fn verify_never_creates_a_box_key() {
    let (_d, home, _) = setup(|t| t);
    let a = bin(&home).arg("verify").assert().code(0);
    assert!(stdout(&a).contains("nothing to verify"), "{}", stdout(&a));
    assert!(!home.join("keys/box.ed25519").exists());
    // With log entries and no key, verify fails and still makes no key.
    let (_d2, home2, _) = registered();
    std::fs::remove_file(home2.join("keys/box.ed25519")).unwrap();
    let a = bin(&home2).arg("verify").assert().code(1);
    assert!(stderr(&a).contains("no box key"), "{}", stderr(&a));
    assert!(!home2.join("keys/box.ed25519").exists());
}

#[test]
fn verify_reports_every_failure_even_for_an_unknown_study() {
    let (_d, home, _) = registered();
    let p = home.join("planlog/leaves.jsonl");
    let text = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, text.replace("signal", "signaL")).unwrap();
    let a = bin(&home)
        .args(["verify", "--study", "nope"])
        .assert()
        .code(1);
    let err = stderr(&a);
    assert!(err.contains("planlog") && err.contains("nope"), "{err}");
}

#[test]
fn check_finds_a_secret_in_a_comment_and_never_echoes_it() {
    let token = format!("ghp_{}", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8");
    let edit = |t: String| {
        let t = t.replacen(
            "cpu_hours = 1.0",
            &format!("cpu_hours = 1.0  # see {token}"),
            1,
        );
        format!(
            "{t}
# old = \"{token}\"
"
        )
    };
    let (_d, home, spec) = setup(edit);
    let text = std::fs::read_to_string(&spec).unwrap();
    let line = |needle: &str| 1 + text.lines().position(|l| l.contains(needle)).unwrap();
    let a = bin(&home)
        .arg("check")
        .arg(&spec)
        .arg("--json")
        .assert()
        .code(1);
    assert!(!stdout(&a).contains(&token));
    let got: Vec<_> = json_out(&a)["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "E-SEC-001")
        .map(|d| {
            (
                d["code"].as_str().unwrap().to_string(),
                d["path"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (
                "E-SEC-001".to_string(),
                format!("line {}", line("cpu_hours"))
            ),
            ("E-SEC-001".to_string(), format!("line {}", line("old ="))),
        ]
    );
}

#[test]
fn content_faults_that_break_the_model_exit_1_with_a_code() {
    type Case = (fn(String) -> String, &'static str, &'static str);
    let cases: [Case; 4] = [
        (
            |t| t.replace("units = 3 ", "units = [3] "),
            "E-TYP-001",
            "inputs[1].units",
        ),
        (
            |t| t.replace("creators = []", "creators = [1]"),
            "E-TYP-001",
            "inputs[0].licence.creators[0]",
        ),
        (
            |t| {
                format!(
                    "\"design.alpha\" = 0.1
{t}"
                )
            },
            "E-KEY-001",
            "<key>",
        ),
        (
            |t| {
                t.replace("operators = []", "operators = [{ name = \"k\", type = \"int\", range = [true], acts_on = \"x\" }]")
            },
            "E-TYP-001",
            "spec",
        ),
    ];
    for (edit, code, path) in cases {
        let (_d, home, spec) = setup(edit);
        let a = bin(&home)
            .arg("check")
            .arg(&spec)
            .arg("--json")
            .assert()
            .code(1);
        let v = json_out(&a);
        let hit = v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["code"] == code)
            .unwrap_or_else(|| panic!("{code} missing: {v}"));
        assert_eq!(hit["path"], path);
        assert!(!hit["instruction"].as_str().unwrap().is_empty());
        assert!(v["estimate"].is_null());
        assert!(stderr(&a).is_empty(), "{}", stderr(&a));
        let a = bin(&home).arg("review").arg(&spec).assert().code(1);
        assert!(stdout(&a).contains(code), "{}", stdout(&a));
    }
}

fn check_out(home: &Path, spec: &Path, code: i32) -> String {
    stdout(&bin(home).arg("check").arg(spec).assert().code(code))
}

fn with_code<'a>(v: &'a Value, code: &str) -> Vec<&'a Value> {
    v["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == code)
        .collect()
}

#[test]
fn an_unfinished_study_is_pending_and_check_exits_3() {
    let (_d, home, spec) = setup(common::confirmatory);
    let box_home = Home::open_at(&home).unwrap();
    // Not registered; registered with no analysis record; an analysis record that promises
    // an estimate the log does not hold yet.
    let all = common::finished(vec![common::arm_a()]);
    for (planted, which) in [
        (&all[..0], "not registered"),
        (&all[..1], "not finished"),
        (&all[1..2], "not finished"),
    ] {
        common::plant(&box_home, "my-pilot", planted);
        let a = bin(&home)
            .arg("check")
            .arg(&spec)
            .arg("--json")
            .assert()
            .code(3);
        let v = json_out(&a);
        assert_eq!(v["pending"], json!(["question.hypotheses[0].estimates"]));
        let w = with_code(&v, "W-REF-001");
        assert_eq!(w.len(), 1, "{v}");
        assert_eq!(w[0]["severity"], "warning");
        assert_eq!(
            w[0]["problem"],
            format!(
                "A value waits on a study that has not finished. Waits on study my-pilot, which is {which}."
            )
        );
        assert!(
            !v["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["severity"] == "error"),
            "{v}"
        );
        // Values pending, no errors: the plan still gets its estimate and review.
        assert!(v["estimate"].is_object());
        let out = check_out(&home, &spec, 3);
        assert!(out.contains("warning[W-REF-001]"), "{out}");
        let a = bin(&home).arg("review").arg(&spec).assert().code(3);
        assert!(stdout(&a).contains("W-REF-001"));
    }
    // The last estimate arrives: the study is finished, and the plan passes.
    common::plant(&box_home, "my-pilot", &all[2..]);
    let a = bin(&home)
        .arg("check")
        .arg(&spec)
        .arg("--json")
        .assert()
        .code(0);
    assert_eq!(json_out(&a)["pending"], json!([]));
    // An error wins over a pending value.
    std::fs::write(
        &spec,
        common::confirmatory(std::fs::read_to_string(TEMPLATE).unwrap())
            .replace("from = \"my-pilot\"", "from = \"later\"")
            .replace("unit = \"ratio\"", "unit = \"\""),
    )
    .unwrap();
    let v = json_out(
        &bin(&home)
            .arg("check")
            .arg(&spec)
            .arg("--json")
            .assert()
            .code(1),
    );
    assert_eq!(v["pending"], json!(["question.hypotheses[0].estimates"]));
}

#[test]
fn the_gate_gives_the_same_verdict_on_every_check() {
    // The median claim sized 3 below the margin: about 93% of units lie at or below it, so
    // some replicates of 15 clusters pass and some do not.
    let near = |t: String| common::confirmatory(t).replace("power_at = -20.0", "power_at = -3.0");
    let (_d, home, spec) = setup(near);
    common::pilot(&Home::open_at(&home).unwrap(), vec![common::arm_a()]);
    let power = |code: i32| {
        let a = bin(&home)
            .arg("check")
            .arg(&spec)
            .arg("--json")
            .assert()
            .code(code);
        json_out(&a)["power"].clone()
    };
    let first = power(1);
    assert_eq!(
        first,
        json!([{
            "hypothesis": "H1", "power": 0.3625, "target": 0.8, "clusters": { "": 15 },
            "needed": 20
        }])
    );
    assert_eq!(power(1), first);
    let sentence =
        "Simulated power 0.36 with 15 clusters in the pooled group; about 20 clusters reach 0.80.";
    assert!(check_out(&home, &spec, 1).contains(sentence));
    let page = stdout(&bin(&home).arg("review").arg(&spec).assert().code(1));
    assert!(
        page.contains(&format!(
            "## Power

- H1: {sentence}
"
        )),
        "{page}"
    );
    // Another spec hash draws other replicates for the same plan.
    let text = std::fs::read_to_string(&spec).unwrap();
    std::fs::write(
        &spec,
        text.replace("population = \"\"", "population = \"texts\""),
    )
    .unwrap();
    let other = power(1);
    assert_ne!(other[0]["power"], first[0]["power"]);
    assert_eq!(other[0]["clusters"], first[0]["clusters"]);
    // Sized 20 below the margin, every replicate passes, and nothing more is needed.
    std::fs::write(
        &spec,
        common::confirmatory(std::fs::read_to_string(TEMPLATE).unwrap()),
    )
    .unwrap();
    assert_eq!(
        power(0),
        json!([{
            "hypothesis": "H1", "power": 1.0, "target": 0.8, "clusters": { "": 15 },
            "needed": null
        }])
    );
    let page = stdout(&bin(&home).arg("review").arg(&spec).assert().code(0));
    assert!(
        page.contains(
            "- H1: Simulated power 1.00 with 15 clusters in the pooled group; the target is 0.80.
"
        ),
        "{page}"
    );
    // Sized just below the margin, half the units lie on each side, and 1,024 clusters do
    // not reach the target: `needed` says so, apart from the null of a plan that passes.
    let edge = |t: String| common::confirmatory(t).replace("power_at = -20.0", "power_at = -0.01");
    std::fs::write(&spec, edge(std::fs::read_to_string(TEMPLATE).unwrap())).unwrap();
    let beyond = power(1);
    assert_eq!(beyond[0]["needed"], ">1024", "{beyond}");
    assert!(
        check_out(&home, &spec, 1).contains("; 1024 clusters do not reach 0.80."),
        "{beyond}"
    );
}

#[test]
fn a_broken_key_fails_only_the_checks_that_need_a_record() {
    let (_d, home, spec) = setup(|t| t);
    let box_home = Home::open_at(&home).unwrap();
    common::pilot(&box_home, vec![common::arm_a()]);
    std::fs::write(home.join("keys/box.ed25519"), b"short").unwrap();
    // An exploratory plan reads no record, so the key is never read.
    assert!(!check_out(&home, &spec, 0).contains("E-CNF-003"));
    let text = common::confirmatory(std::fs::read_to_string(TEMPLATE).unwrap());
    std::fs::write(&spec, &text).unwrap();
    let out = check_out(&home, &spec, 1);
    assert!(out.contains("error[E-CNF-003]"), "{out}");
    assert!(
        out.contains(
            "The box cannot read the records of study my-pilot: key file has 5 bytes, expected 32."
        ),
        "{out}"
    );
    // A study with no log in the box needs no key to be not registered. The stream still
    // needs it: each study log's register record must be checked against the stream.
    std::fs::write(
        &spec,
        text.replace("from = \"my-pilot\"", "from = \"later\""),
    )
    .unwrap();
    let out = check_out(&home, &spec, 1);
    assert!(
        out.contains("Waits on study later, which is not registered."),
        "{out}"
    );
    assert!(
        out.contains(
            "The planlog stream cannot be read: study my-pilot: key file has 5 bytes, expected 32."
        ),
        "{out}"
    );
    // With no study log in the box, no key is read at all.
    std::fs::rename(home.join("studies/my-pilot"), home.join("moved-pilot")).unwrap();
    assert!(check_out(&home, &spec, 3).contains("Waits on study later, which is not registered."));
}

#[test]
fn logs_without_a_box_key_cannot_answer() {
    let (_d, home, spec) = setup(common::confirmatory);
    Home::open_at(&home).unwrap();
    std::fs::create_dir(home.join("studies/my-pilot")).unwrap();
    std::fs::write(home.join("studies/my-pilot/records.jsonl"), "{}\n").unwrap();
    let out = check_out(&home, &spec, 1);
    assert!(
        out.contains("The box cannot read the records of study my-pilot: the box key is missing."),
        "{out}"
    );
    // With a key, the same log is not a whole record.
    keys::load_or_create(&Home::open_at(&home).unwrap()).unwrap();
    let out = check_out(&home, &spec, 1);
    assert!(
        out.contains(
            "The box cannot read the records of study my-pilot: line 1 is not a whole record."
        ),
        "{out}"
    );
}

#[test]
fn a_log_without_a_register_record_is_not_a_registered_study() {
    let (_d, home, spec) = setup(common::confirmatory);
    let box_home = Home::open_at(&home).unwrap();
    // A log whose only record does not parse as a register record.
    common::plant(
        &box_home,
        "my-pilot",
        &[("register", json!({ "spec_hash": "x" }))],
    );
    let out = check_out(&home, &spec, 3);
    assert!(
        out.contains("Waits on study my-pilot, which is not registered."),
        "{out}"
    );
}

// ---- stream ----

fn check_json(home: &Path, spec: &Path, code: i32) -> Value {
    json_out(
        &bin(home)
            .arg("check")
            .arg(spec)
            .arg("--json")
            .assert()
            .code(code),
    )
}

#[test]
fn check_shows_the_slot_and_level_the_stream_would_give() {
    let (_d, home, spec) = setup(common::confirmatory);
    let box_home = Home::open_at(&home).unwrap();
    common::pilot(&box_home, vec![common::arm_a()]);
    let out = check_out(&home, &spec, 0);
    assert!(
        out.contains("\nstream: slot 1, level 0.005\nspec: "),
        "{out}"
    );
    let v = check_json(&home, &spec, 0);
    assert_eq!(
        v["stream"],
        json!({ "log": "planlog", "slot": 1, "level": 0.005000000000000001 })
    );
    // A check writes nothing to the stream.
    assert!(!home.join("stream/planlog.jsonl").exists());
    // Once a study holds slot 1, the next is slot 2; the review page shows it too.
    let key = keys::load_or_create(&box_home).unwrap();
    stream::Log::of(&box_home, LogName::Plan)
        .open(&key, "other", 0.0)
        .unwrap();
    assert!(check_out(&home, &spec, 0).contains("\nstream: slot 2, level 0.005\n"));
    let page = stdout(&bin(&home).arg("review").arg(&spec).assert().code(0));
    assert!(
        page.contains("\n## Level\n\n- stream: slot 2, level 0.005\n\n## Power\n"),
        "{page}"
    );
    // A test study reads the test stream, which is still empty.
    let text = std::fs::read_to_string(&spec).unwrap();
    std::fs::write(&spec, text.replace("test_log = false", "test_log = true")).unwrap();
    assert_eq!(
        check_json(&home, &spec, 0)["stream"],
        json!({ "log": "testlog", "slot": 1, "level": 0.005000000000000001 })
    );
    // An exploratory plan takes no slot.
    let (_d, home, spec) = setup(|t| t);
    assert!(!check_out(&home, &spec, 0).contains("stream:"));
    assert_eq!(check_json(&home, &spec, 0)["stream"], Value::Null);
    let page = stdout(&bin(&home).arg("review").arg(&spec).assert().code(0));
    assert!(page.contains("\n## Level\n\n- none\n"), "{page}");
}

#[test]
fn check_shows_where_determinism_comes_from() {
    // A confirmatory plan that declares determinism and pins the payload and environment
    // my-pilot ran, in a box where my-pilot's check passed, or holds none.
    let i = common::instrument();
    let pinned = |t: String| {
        common::confirmatory(t)
            .replace("# deterministic = true", "deterministic = true")
            .replace("payload = \"\"", &format!("payload = \"{}\"", i.payload))
            .replace(
                "environment = \"\"",
                &format!("environment = \"{}\"", i.environment),
            )
    };
    let passed = ignorarium::derived::Recheck {
        rows: 2,
        mismatched: vec![],
        uncompared: vec![],
        carried_from: None,
    };
    let checked = |edit: &dyn Fn(String) -> String, recheck| {
        let (d, home, spec) = setup(edit);
        let pilot = common::finished_with(i.clone(), recheck, vec![common::arm_a()]);
        common::plant(&Home::open_at(&home).unwrap(), "my-pilot", &pilot);
        (check_out(&home, &spec, 0), check_json(&home, &spec, 0), d)
    };
    let (out, v, _d) = checked(&pinned, Some(passed));
    assert!(
        out.contains("\n\ndeterminism: carried from my-pilot\nstream: slot 1, level 0.005\n"),
        "{out}"
    );
    assert_eq!(
        v["recheck"],
        json!({ "carried_from": "my-pilot", "rows": 0 })
    );
    assert_eq!(v["estimate"]["rows"], 30);
    // A source without a passed check: the plan runs its own, one copy per condition id.
    let (out, v, _d) = checked(&pinned, None);
    assert!(
        out.contains("\n\ndeterminism: 2 recheck rows\nstream: slot 1, level 0.005\n"),
        "{out}"
    );
    assert_eq!(v["recheck"], json!({ "carried_from": null, "rows": 2 }));
    assert_eq!(v["estimate"]["rows"], 32);
    // A plan without determinism has no recheck.
    let (out, v, _d) = checked(&common::confirmatory, None);
    assert!(!out.contains("determinism"), "{out}");
    assert_eq!(v["recheck"], Value::Null);
    assert_eq!(v["estimate"]["rows"], 30);
}

#[test]
fn check_refuses_a_plan_beyond_the_tenth_slot() {
    let (_d, home, spec) = setup(common::confirmatory);
    let box_home = Home::open_at(&home).unwrap();
    common::pilot(&box_home, vec![common::arm_a()]);
    let key = keys::load_or_create(&box_home).unwrap();
    let log = stream::Log::of(&box_home, LogName::Plan);
    for i in 1..=10 {
        log.open(&key, &format!("s{i:02}"), 0.0).unwrap();
    }
    let v = check_json(&home, &spec, 1);
    let errors: Vec<&Value> = (v["diagnostics"].as_array().unwrap().iter())
        .filter(|d| d["severity"] == "error")
        .collect();
    assert_eq!(errors.len(), 1, "{v}");
    assert_eq!(
        (&errors[0]["code"], &errors[0]["path"]),
        (&json!("E-STR-001"), &json!("study.mode"))
    );
    assert_eq!(
        errors[0]["problem"],
        "The stream gives this confirmatory study no level. Slot 11 is beyond the 10 slots of the planlog stream."
    );
    assert_eq!(
        v["stream"],
        json!({ "log": "planlog", "slot": 11, "level": 0.0 })
    );
    assert_eq!(v["power"], json!([]));
    let out = check_out(&home, &spec, 1);
    assert!(out.contains("error[E-STR-001]"), "{out}");
    assert!(out.contains("\nstream: slot 11, level 0\n"), "{out}");
    // A damaged stream log fails the check closed.
    std::fs::write(home.join("stream/planlog.jsonl"), "{}\n").unwrap();
    let v = check_json(&home, &spec, 1);
    assert_eq!(
        with_code(&v, "E-STR-001")[0]["problem"],
        "The stream gives this confirmatory study no level. The planlog stream cannot be read: the log fails verification: line 1 is not a whole record."
    );
    assert_eq!(v["stream"], Value::Null);
}

#[test]
fn a_study_reference_never_names_a_path() {
    let (d, home, _) = setup(|t| t);
    let box_home = Home::open_at(&home).unwrap();
    // A record log where each of these ids would lead: beside the studies folder, in it, in a
    // folder with a capital letter, and one folder down. Without a key, a log that is read
    // is an error, so `Ok(None)` shows that none was read.
    for (id, folder) in [
        ("../outside", "outside"),
        ("", "studies"),
        ("My-Pilot", "studies/My-Pilot"),
        ("a/b", "studies/a/b"),
    ] {
        std::fs::create_dir_all(home.join(folder)).unwrap();
        std::fs::write(home.join(folder).join("records.jsonl"), "{}\n").unwrap();
        let records = HomeRecords {
            home: box_home.clone(),
            key: Ok(None),
        };
        assert_eq!(records.study(id), Ok(None), "{id}");
    }
    // The same reader does read a study id's log.
    std::fs::create_dir_all(home.join("studies/my-pilot")).unwrap();
    std::fs::write(home.join("studies/my-pilot/records.jsonl"), "{}\n").unwrap();
    let records = HomeRecords {
        home: box_home,
        key: Ok(None),
    };
    assert_eq!(
        records.study("my-pilot"),
        Err("the box key is missing".to_string())
    );
    drop(d);
}

#[test]
fn a_study_folder_that_cannot_be_read_is_an_error() {
    use std::os::unix::fs::PermissionsExt;
    let (_d, home, _) = setup(|t| t);
    let box_home = Home::open_at(&home).unwrap();
    common::pilot(&box_home, vec![common::arm_a()]);
    let folder = home.join("studies/my-pilot");
    let mode = |m| std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(m)).unwrap();
    mode(0o000);
    if folder.join("records.jsonl").symlink_metadata().is_ok() {
        mode(0o755);
        println!("skip unreadable study folder: this user reads a mode-000 folder (root)");
        return;
    }
    let key = keys::load(&box_home).unwrap().map(|k| k.verifying_key());
    let got = HomeRecords {
        home: box_home,
        key: Ok(key),
    }
    .study("my-pilot");
    mode(0o755);
    // Not "not registered": the box cannot tell.
    let e = got.unwrap_err();
    assert!(e.contains("Permission denied"), "{e}");
}

#[test]
fn a_log_that_holds_another_studys_records_is_an_error() {
    let (_d, home, _) = setup(|t| t);
    let box_home = Home::open_at(&home).unwrap();
    // A finished study's log, copied under another study's name.
    common::plant(&box_home, "first", &common::finished(vec![common::arm_a()]));
    std::fs::create_dir_all(home.join("studies/my-pilot")).unwrap();
    std::fs::copy(
        home.join("studies/first/records.jsonl"),
        home.join("studies/my-pilot/records.jsonl"),
    )
    .unwrap();
    let key = keys::load(&box_home).unwrap().map(|k| k.verifying_key());
    let records = HomeRecords {
        home: box_home,
        key: Ok(key),
    };
    assert_eq!(
        records.study("my-pilot"),
        Err("record 0 belongs to study first".to_string())
    );
    assert!(records.study("first").unwrap().is_some());
}

// ---- import ----

fn blob_lines(home: &Path) -> Vec<Value> {
    std::fs::read_to_string(home.join("store/imports.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn import_prints_digests_and_signs_one_record_per_file() {
    let (d, home, _) = setup(|t| t);
    let (a, b) = (d.path().join("a.txt"), d.path().join("b.bin"));
    std::fs::write(&a, "Hello  World").unwrap();
    std::fs::write(&b, [0xff, 0xfe, 0x00]).unwrap();
    let out = bin(&home)
        .args(["import", "--source", "the pilot corpus"])
        .args([&a, &b])
        .assert()
        .code(0);
    let sha = ignorarium::canon::sha256_hex;
    let (da, db) = (sha(b"Hello  World"), sha(&[0xff, 0xfe, 0x00]));
    assert_eq!(
        stdout(&out),
        format!(
            "{da}  {}
{db}  {}
",
            a.display(),
            b.display()
        )
    );
    assert_eq!(
        std::fs::read(home.join("store").join(&da)).unwrap(),
        b"Hello  World"
    );
    let key = ignorarium::keys::load(&Home::open_at(&home).unwrap())
        .unwrap()
        .unwrap()
        .verifying_key();
    let recs = ignorarium::records::read_all(&home.join("store/imports.jsonl"), &key).unwrap();
    assert_eq!(recs.len(), 2);
    assert_eq!(
        (recs[0].study.as_str(), recs[0].kind.as_str()),
        ("store", "import")
    );
    assert_eq!(recs[0].row, None);
    assert_eq!(
        recs[0].body,
        serde_json::json!({
            "digest": da, "size": 12, "source": "the pilot corpus",
            "normalized_sha256": sha(b"hello world"), "file": "a.txt"
        })
    );
    assert_eq!(recs[1].body["digest"], db);
    assert_eq!(recs[1].body["file"], "b.bin");
    assert_eq!(recs[1].body["normalized_sha256"], "");
    assert_eq!(recs[1].seq, 1);
    let ev = events(&home);
    assert_eq!(ev[0]["name"], "import");
    assert_eq!(ev[0]["attrs"]["exit"], 0);
}

#[test]
fn import_writes_no_meta_when_its_record_cannot_be_signed() {
    let (d, home, _) = setup(|t| t);
    let (a, b) = (d.path().join("a.txt"), d.path().join("b.txt"));
    std::fs::write(&a, "first").unwrap();
    std::fs::write(&b, "second").unwrap();
    bin(&home)
        .args(["import", "--source", "s"])
        .arg(&a)
        .assert()
        .code(0);
    // A torn last line, as a crash in mid-write leaves it: no record can be appended.
    let log = home.join("store/imports.jsonl");
    let whole = std::fs::read(&log).unwrap();
    std::fs::write(&log, [&whole[..], b"{\"rec"].concat()).unwrap();
    bin(&home)
        .args(["import", "--source", "s"])
        .arg(&b)
        .assert()
        .code(2);
    // A meta would tell a later register that the blob already has its record.
    let digest = ignorarium::canon::sha256_hex(b"second");
    let meta = home.join("store/meta").join(format!("{digest}.json"));
    assert!(!meta.exists());
    // With the log whole again, the import signs its record, then writes the meta.
    std::fs::write(&log, &whole).unwrap();
    bin(&home)
        .args(["import", "--source", "s"])
        .arg(&b)
        .assert()
        .code(0);
    assert!(meta.exists());
    let key = ignorarium::keys::load(&Home::open_at(&home).unwrap())
        .unwrap()
        .unwrap()
        .verifying_key();
    let recs = ignorarium::records::read_all(&log, &key).unwrap();
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[1].body["digest"], digest.as_str());
}

#[test]
fn import_refuses_a_bad_file_before_it_adds_anything() {
    let (d, home, _) = setup(|t| t);
    let good = d.path().join("good.txt");
    std::fs::write(&good, "x").unwrap();
    for bad in [d.path().join("missing.txt"), d.path().to_path_buf()] {
        let a = bin(&home)
            .args(["import", "--source", "s"])
            .arg(&good)
            .arg(&bad)
            .assert()
            .code(2);
        assert!(
            stderr(&a).contains(&bad.display().to_string()),
            "{}",
            stderr(&a)
        );
        assert!(
            !home
                .join("store")
                .join(ignorarium::canon::sha256_hex(b"x"))
                .exists()
        );
        assert!(blob_lines(&home).is_empty());
    }
}

#[cfg(unix)]
#[test]
fn import_refuses_a_symlink() {
    let (d, home, _) = setup(|t| t);
    let real = d.path().join("real.txt");
    std::fs::write(&real, "x").unwrap();
    let link = d.path().join("link.txt");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let a = bin(&home)
        .args(["import", "--source", "s"])
        .arg(&link)
        .assert()
        .code(2);
    assert!(stderr(&a).contains("not a regular file"), "{}", stderr(&a));
    assert!(blob_lines(&home).is_empty());
}

#[test]
fn import_refuses_a_source_that_looks_like_a_secret_and_does_not_echo_it() {
    let (d, home, _) = setup(|t| t);
    let f = d.path().join("a.txt");
    std::fs::write(&f, "x").unwrap();
    let secret = &format!("AKIA{}", "ABCDEFGHIJKLMNOP");
    let a = bin(&home)
        .args(["import", "--source", &format!("bucket key {secret}")])
        .arg(&f)
        .assert()
        .code(1);
    let err = stderr(&a);
    assert!(err.contains("aws-key"), "{err}");
    assert!(!err.contains(secret), "{err}");
    assert!(!stdout(&a).contains(secret));
    assert!(blob_lines(&home).is_empty());
    assert!(
        !home
            .join("store")
            .join(ignorarium::canon::sha256_hex(b"x"))
            .exists()
    );
    // The event log keeps no copy of the secret either.
    let log = std::fs::read_to_string(home.join("events.jsonl")).unwrap_or_default();
    assert!(!log.contains(secret), "{log}");
}

#[test]
fn import_needs_a_source() {
    let (d, home, _) = setup(|t| t);
    let f = d.path().join("a.txt");
    std::fs::write(&f, "x").unwrap();
    bin(&home).arg("import").arg(&f).assert().code(2);
    let a = bin(&home)
        .args(["import", "--source", ""])
        .arg(&f)
        .assert()
        .code(2);
    assert!(stderr(&a).contains("--source needs text"), "{}", stderr(&a));
}

#[test]
fn a_whitespace_only_source_is_refused() {
    let (d, home, _) = setup(|t| t);
    let f = d.path().join("a.txt");
    std::fs::write(&f, "x").unwrap();
    let a = bin(&home)
        .args(["import", "--source", "  "])
        .arg(&f)
        .assert()
        .code(2);
    assert!(stderr(&a).contains("--source needs text"), "{}", stderr(&a));
}

#[test]
fn import_refuses_a_home_path_in_the_source_and_a_secret_in_the_file_name() {
    let (d, home, _) = setup(|t| t);
    let f = d.path().join("a.txt");
    std::fs::write(&f, "x").unwrap();
    for src in [
        format!("copied from /{}/{}/data", "home", "alice"),
        format!("copied from /{}/{}/data", "Users", "alice"),
        format!("copied from C:\\{}\\{}\\data", "Users", "alice"),
    ] {
        let a = bin(&home)
            .args(["import", "--source", &src])
            .arg(&f)
            .assert()
            .code(1);
        assert!(stderr(&a).contains("home-path"), "{src}: {}", stderr(&a));
        assert!(!stderr(&a).contains("alice"), "{}", stderr(&a));
    }
    // A placeholder is not a path.
    bin(&home)
        .args(["import", "--source", "kept under /home/<name>/data"])
        .arg(&f)
        .assert()
        .code(0);
    let secret = format!("AKIA{}.txt", "ABCDEFGHIJKLMNOP");
    let g = d.path().join(&secret);
    std::fs::write(&g, "y").unwrap();
    let a = bin(&home)
        .args(["import", "--source", "s"])
        .arg(&g)
        .assert()
        .code(1);
    assert!(stderr(&a).contains("aws-key"), "{}", stderr(&a));
    assert!(!stderr(&a).contains(&secret), "{}", stderr(&a));
}

#[test]
fn a_signed_import_record_holds_the_file_name_only() {
    let (d, home, _) = setup(|t| t);
    let sub = d.path().join("deep/er");
    std::fs::create_dir_all(&sub).unwrap();
    let f = sub.join("a.txt");
    std::fs::write(&f, "x").unwrap();
    let a = bin(&home)
        .args(["import", "--source", "s"])
        .arg(&f)
        .assert()
        .code(0);
    // The path as given is printed, and kept out of the record.
    assert!(stdout(&a).contains("deep"), "{}", stdout(&a));
    let raw = std::fs::read_to_string(home.join("store/imports.jsonl")).unwrap();
    assert!(!raw.contains("deep"), "{raw}");
}

#[test]
fn check_creates_no_store_folder() {
    let (_d, home, spec) = setup(no_power);
    bin(&home).arg("check").arg(&spec).assert().code(0);
    assert!(!home.join("store").exists());
}

#[test]
fn register_accepts_an_input_that_only_the_store_holds() {
    let (d, home, _) = setup(|t| t);
    let f = d.path().join("corpus.txt");
    std::fs::write(&f, "Stored  Words").unwrap();
    bin(&home)
        .args(["import", "--source", "s"])
        .arg(&f)
        .assert()
        .code(0);
    let digest = ignorarium::canon::sha256_hex(b"Stored  Words");
    let spec = d.path().join("stored-spec.toml");
    let text = std::fs::read_to_string(TEMPLATE).unwrap();
    let pin = format!("sha256 = \"{digest}\"");
    assert!(text.contains("sha256 = \"\""));
    std::fs::write(&spec, text.replacen("sha256 = \"\"", &pin, 1)).unwrap();
    give_payload(&spec);
    let h = Home::open_at(&home).unwrap();
    let typed = format!("{}\n", &ignorarium::spec::load(&spec).unwrap().hash[7..15]);
    let mut out = Vec::new();
    let done = ignorarium::cli::register::run(
        &spec,
        &h,
        &mut std::io::Cursor::new(typed.into_bytes()),
        &mut out,
        true,
    )
    .unwrap_or_else(|e| panic!("{}", e.msg));
    assert_eq!(done.code, 0);
    let leaves = leaf_lines(&home, "planlog");
    assert_eq!(
        leaves[0]["data_hashes"],
        serde_json::json!([[digest, ignorarium::canon::sha256_hex(b"stored words")]])
    );
    common::unseal(&home);
}
