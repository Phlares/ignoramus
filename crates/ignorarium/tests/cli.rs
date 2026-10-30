mod common;

use assert_cmd::Command;
use ignorarium::home::Home;
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
            "brief", "template", "fill", "check", "review", "register", "run", "status", "report"
        ]
    );
    for s in steps {
        for key in ["title", "does", "produces", "command"] {
            assert!(!s[key].as_str().unwrap().is_empty(), "{s}");
        }
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

/// Register the template through the library, then return the box.
fn registered() -> (TempDir, PathBuf, PathBuf) {
    let (d, home, spec) = setup(|t| t);
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
            |t| {
                t.replace(
                    "[design.confirmation]
clusters = []",
                    "[design.confirmation]
clusters = [1]",
                )
            },
            "E-TYP-001",
            "design.confirmation.clusters[0]",
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

#[test]
fn check_refuses_a_made_up_variance_record() {
    let (_d, home, spec) = setup(|t| {
        t.replace("mode = \"exploratory\"", "mode = \"confirmatory\"")
            .replace(
                "source = \"stated\"",
                &format!("source = \"record:{}\"", "0".repeat(64)),
            )
    });
    let a = bin(&home).arg("check").arg(&spec).assert().code(1);
    let out = stdout(&a);
    assert!(out.contains("error[E-CNF-003]"), "{out}");
    assert!(
        out.contains("No record in this box has this hash."),
        "{out}"
    );
}

/// The template as a confirmatory plan that states variance 4 and takes it from `source`.
fn confirmatory_from(source: &str) -> String {
    std::fs::read_to_string(TEMPLATE)
        .unwrap()
        .replace("mode = \"exploratory\"", "mode = \"confirmatory\"")
        .replace("value = 0.0", "value = 4.0")
        .replace("source = \"stated\"", &format!("source = \"{source}\""))
}

fn check_out(home: &Path, spec: &Path, code: i32) -> String {
    stdout(&bin(home).arg("check").arg(spec).assert().code(code))
}

#[test]
fn check_accepts_a_variance_record_that_holds_the_stated_value() {
    let (_d, home, spec) = setup(|t| t);
    let h = common::plant_estimate(&Home::open_at(&home).unwrap(), "ppl-ratio", 4.0);
    std::fs::write(&spec, confirmatory_from(&format!("record:{h}"))).unwrap();
    let out = check_out(&home, &spec, 1);
    assert!(!out.contains("E-CNF-003"), "{out}");
}

#[test]
fn check_refuses_a_variance_record_with_another_value() {
    let (_d, home, spec) = setup(|t| t);
    let h = common::plant_estimate(&Home::open_at(&home).unwrap(), "ppl-ratio", 5.0);
    std::fs::write(&spec, confirmatory_from(&format!("record:{h}"))).unwrap();
    let out = check_out(&home, &spec, 1);
    assert!(out.contains("error[E-CNF-003]"), "{out}");
    assert!(
        out.contains("The record holds variance 5, and the plan states 4."),
        "{out}"
    );
}

#[test]
fn a_broken_key_fails_only_the_checks_that_need_a_record() {
    let (_d, home, spec) = setup(|t| t);
    Home::open_at(&home).unwrap();
    std::fs::write(home.join("keys/box.ed25519"), b"short").unwrap();
    // An exploratory plan reads no record, so the key is never read.
    assert!(!check_out(&home, &spec, 0).contains("E-CNF-003"));
    std::fs::write(
        &spec,
        confirmatory_from(&format!("record:{}", "0".repeat(64))),
    )
    .unwrap();
    let out = check_out(&home, &spec, 1);
    assert!(out.contains("error[E-CNF-003]"), "{out}");
    assert!(
        out.contains("The box cannot read its records: key file has 5 bytes"),
        "{out}"
    );
}

#[test]
fn logs_without_a_box_key_cannot_answer() {
    let (_d, home, spec) = setup(|t| t);
    Home::open_at(&home).unwrap();
    std::fs::create_dir(home.join("studies/old")).unwrap();
    std::fs::write(home.join("studies/old/records.jsonl"), "{}\n").unwrap();
    std::fs::write(
        &spec,
        confirmatory_from(&format!("record:{}", "0".repeat(64))),
    )
    .unwrap();
    let out = check_out(&home, &spec, 1);
    assert!(
        out.contains("The box cannot read its records: the box key is missing"),
        "{out}"
    );
}
