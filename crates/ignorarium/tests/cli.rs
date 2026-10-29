use assert_cmd::Command;
use serde_json::Value;
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

/// Drops the alpha line so the default fills it.
fn no_alpha(t: String) -> String {
    t.replace("alpha = 0.005\n", "")
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
            "brief", "template", "fill", "check", "review", "register", "run", "report"
        ]
    );
    for s in steps {
        for key in ["title", "does", "produces", "command"] {
            assert!(!s[key].as_str().unwrap().is_empty(), "{s}");
        }
    }
    for id in ["run", "report"] {
        let s = steps.iter().find(|s| s["id"] == id).unwrap();
        assert!(s["does"].as_str().unwrap().contains("not available yet"));
    }
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
    assert!(out.contains("ignorarium check"));
}

#[test]
fn check_passes_the_template_and_prints_the_estimate() {
    let (_d, home, spec) = setup(no_alpha);
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
        "design.alpha = 0.005",
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
    let (_d, home, spec) = setup(no_alpha);
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
    assert_eq!(codes, ["W-LIC-001", "W-HSH-001"]);
    assert_eq!(v["estimate"]["rows"], 2);
    assert_eq!(v["estimate"]["label"], "signal");
    assert_eq!(v["estimate"]["long_job"], false);
    assert!(
        v["defaults"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["path"] == "design.alpha" && d["value"] == 0.005)
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
fn review_of_a_failing_plan_lists_the_errors() {
    let (_d, home, spec) = setup(no_unit);
    let a = bin(&home).arg("review").arg(&spec).assert().code(1);
    let out = stdout(&a);
    let warnings = &out[out.find("## Warnings").unwrap()..];
    assert!(warnings.contains("### Errors"), "{out}");
    assert!(warnings.contains("E-OUT-001"));
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
            text.replace("sha256 = \"\"", &format!("sha256 = \"{sha}\"")),
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
    assert!(!out.contains("Plan 2"));
    assert!(out.matches("not available yet").count() >= 2);
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
