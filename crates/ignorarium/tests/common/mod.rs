//! A registered study in a temp home, shared by the run tests.
#![allow(dead_code)]
use ignorarium::cli::register;
use ignorarium::home::Home;
use ignorarium::{keys, records, spec};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub const PAYLOAD: &str =
    "#!/bin/sh\nprintf '%s %s %s' \"$2\" \"$4\" \"$6\" > \"$IGNORARIUM_OUT/result\"\n";

pub struct Lab {
    pub _dir: TempDir,
    pub home: Home,
    pub root: PathBuf,
}

pub fn lab() -> Lab {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let home = Home::open_at(&root.join("home")).unwrap();
    std::fs::write(home.root.join("machine.toml"), "max_level = 1\n").unwrap();
    Lab {
        _dir: dir,
        home,
        root,
    }
}

/// Register a study with `register::run`, then give it a payload.
/// `before` lists files (relative to the study folder) present at registration.
pub fn register_study(
    lab: &Lab,
    id: &str,
    edit: impl Fn(String) -> String,
    before: &[(&str, &str)],
    payload: &str,
) -> PathBuf {
    let work = lab.root.join(format!("work-{id}"));
    std::fs::create_dir_all(work.join("data")).unwrap();
    for (p, c) in before {
        std::fs::write(work.join(p), c).unwrap();
    }
    let text = std::fs::read_to_string("../../templates/numeric-ablation.toml").unwrap();
    let text = text
        .replace("id = \"my-study\"", &format!("id = \"{id}\""))
        .replace("min_isolation = 1", "min_isolation = 0")
        .replace("units_per_cluster = 1", "units_per_cluster = 3");
    let spec_path = work.join("spec.toml");
    std::fs::write(&spec_path, edit(text)).unwrap();
    let hash = spec::load(&spec_path).unwrap().hash;
    let typed = format!("{}\n", &hash["sha256:".len()..][..8]);
    let mut out = Vec::new();
    let mut input = Cursor::new(typed.into_bytes());
    let done = register::run(&spec_path, &lab.home, &mut input, &mut out, true)
        .unwrap_or_else(|e| panic!("{}: {}", e.msg, String::from_utf8_lossy(&out)));
    assert_eq!(done.code, 0, "{}", String::from_utf8_lossy(&out));
    let dir = lab.home.study_dir(id);
    std::fs::create_dir_all(dir.join("payload")).unwrap();
    write_exec(&dir.join("payload/run"), payload);
    dir
}

/// A study whose one input is `data/example.txt`.
pub fn plain(lab: &Lab, id: &str) -> PathBuf {
    with_payload(lab, id, PAYLOAD)
}

/// `plain` with another payload.
pub fn with_payload(lab: &Lab, id: &str, payload: &str) -> PathBuf {
    edited(lab, id, |t| t, payload)
}

/// `with_payload` with the spec text changed by `edit` before registration.
pub fn edited(lab: &Lab, id: &str, edit: impl Fn(String) -> String, payload: &str) -> PathBuf {
    let dir = register_study(
        lab,
        id,
        edit,
        &[("data/example.txt", "calibration")],
        payload,
    );
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("data/example.txt"), "calibration").unwrap();
    dir
}

/// Every verified record of the study, in order.
pub fn records_of(lab: &Lab, id: &str) -> Vec<records::Record> {
    let key = keys::load(&lab.home).unwrap().unwrap();
    let path = lab.home.study_dir(id).join("records.jsonl");
    records::read_all(&path, &key.verifying_key()).unwrap()
}

pub fn kind_of(lab: &Lab, id: &str, kind: &str) -> Vec<records::Record> {
    records_of(lab, id)
        .into_iter()
        .filter(|r| r.kind == kind)
        .collect()
}

pub fn rows_of(lab: &Lab, id: &str) -> Vec<records::Record> {
    kind_of(lab, id, "row")
}

pub fn indices(lab: &Lab, id: &str) -> Vec<u64> {
    rows_of(lab, id).iter().map(|r| r.row.unwrap()).collect()
}

/// The JSON lines of `dir/<name>`; none when it is absent.
pub fn lines_of(dir: &Path, name: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join(name))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

pub fn events_of(dir: &Path) -> Vec<serde_json::Value> {
    lines_of(dir, "events.jsonl")
}

pub fn named<'a>(events: &'a [serde_json::Value], name: &str) -> Vec<&'a serde_json::Value> {
    events.iter().filter(|e| e["name"] == name).collect()
}

const CHILD: &str = "IGNORARIUM_TEST_CHILD";

/// True inside the child that `in_child(name, ..)` starts.
pub fn is_child(name: &str) -> bool {
    std::env::var_os(CHILD).is_some_and(|v| v == name)
}

/// Run test `name` of this test binary again in a child process with `cwd` and `envs`.
/// Process-wide state such as `PATH` or the working folder then changes for that test
/// alone. True when the one test ran and passed.
pub fn in_child(name: &str, cwd: &Path, envs: &[(&str, &std::ffi::OsStr)]) -> bool {
    let mut c = std::process::Command::new(std::env::current_exe().unwrap());
    c.args([name, "--exact"]).current_dir(cwd).env(CHILD, name);
    for (k, v) in envs {
        c.env(k, v);
    }
    let out = c.output().unwrap();
    out.status.success() && String::from_utf8_lossy(&out.stdout).contains(" 1 passed")
}

/// Write an executable file without this process holding a write descriptor to it.
/// A thread that forks while such a descriptor is open hands it to its child, and an
/// `exec` of the file then fails with ETXTBSY. A child process writes the file instead.
pub fn write_exec(path: &Path, text: &str) {
    use std::io::Write;
    let mut child = std::process::Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success(), "cannot write {path:?}");
}
