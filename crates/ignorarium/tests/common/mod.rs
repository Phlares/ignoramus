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

/// `/proc/<pid>/stat` state letter, or `None` once the process is reaped.
pub fn proc_state(pid: u64) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.trim_start().chars().next()
}

/// Wait until the box's row file names row `row` and that row has written `started` to its
/// output, then return the file.
pub fn wait_for_row_file(dir: &Path, row: u64) -> serde_json::Value {
    let t = std::time::Instant::now();
    loop {
        let file = std::fs::read(dir.join("run/row.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
        if let Some(f) = file.filter(|f| f["row"] == row)
            && dir.join(format!("run/rows/{row}/out/started")).exists()
        {
            return f;
        }
        assert!(
            t.elapsed() < std::time::Duration::from_secs(20),
            "row {row} never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

const BOX_HOME: &str = "IGNORARIUM_TEST_BOX_HOME";
const BOX_STUDY: &str = "IGNORARIUM_TEST_BOX_STUDY";

/// Inside the child box that `kill_box_at` starts: its home and the study it runs.
pub fn boxed_study() -> Option<(Home, String)> {
    let root = std::env::var_os(BOX_HOME)?;
    let id = std::env::var(BOX_STUDY).ok()?;
    Some((Home::open_at(Path::new(&root)).unwrap(), id))
}

/// Run study `id` in a child box, this binary's test `child`, kill the box with SIGKILL once
/// row `row` runs, and return the row file it left.
pub fn kill_box_at(lab: &Lab, child: &str, id: &str, row: u64) -> serde_json::Value {
    use std::process::Stdio;
    let mut boxed = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", child, "--nocapture"])
        .env(BOX_HOME, &lab.home.root)
        .env(BOX_STUDY, id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let file = wait_for_row_file(&lab.home.study_dir(id), row);
    boxed.kill().unwrap();
    boxed.wait().unwrap();
    file
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

/// Set to `1` (CI does) to make every level 1 skip a failure.
pub const REQUIRE_LEVEL1: &str = "IGNORARIUM_REQUIRE_LEVEL1";

/// True, once the reason is printed, when this host cannot run level 1, so the test
/// returns early. With `REQUIRE_LEVEL1=1` such a host fails the test instead.
pub fn skip_level1() -> bool {
    let Some(why) = no_level1() else {
        return false;
    };
    let required = std::env::var_os(REQUIRE_LEVEL1).is_some_and(|v| v == "1");
    assert!(!required, "level 1 is required here, but: {why}");
    println!("skip level1: {why}");
    true
}

/// WSL's binfmt_misc handler for Windows programs.
pub const WSL_INTEROP: &str = "/proc/sys/fs/binfmt_misc/WSLInterop";

/// Whether WSL's interop handler has the `F` flag; `None` when the host has no handler.
pub fn interop_f_flag() -> Option<bool> {
    let entry = std::fs::read_to_string(WSL_INTEROP).ok()?;
    let flags = entry.lines().find_map(|l| l.strip_prefix("flags:"));
    Some(flags.is_some_and(|f| f.contains('F')))
}

/// Why bubblewrap cannot run a level 1 sandbox on this host, or `None` when it can.
/// Found by calling `bwrap` directly, never through the box's code, so a fault in that
/// code fails the level 1 tests instead of skipping them. A WSL interop handler with the
/// `F` flag makes level 1 refuse the host, as the probe does.
pub fn no_level1() -> Option<String> {
    if interop_f_flag() == Some(true) {
        return Some(format!("{WSL_INTEROP} has the F flag"));
    }
    let run = |args: &[&str]| std::process::Command::new("bwrap").args(args).output();
    let version = match run(&["--version"]) {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Err(e) => return Some(format!("cannot run bwrap: {e}")),
    };
    let number = |p: Option<&str>| p?.parse::<u32>().ok();
    let mut parts = version.strip_prefix("bubblewrap ").unwrap_or("").split('.');
    match (number(parts.next()), number(parts.next())) {
        (Some(major), Some(minor)) if (major, minor) >= (0, 8) => {}
        _ => return Some(format!("{version:?} is not bubblewrap 0.8 or later")),
    }
    let namespaces = [
        "--unshare-user",
        "--unshare-ipc",
        "--unshare-pid",
        "--unshare-net",
        "--unshare-uts",
        "--unshare-cgroup",
        "--disable-userns",
        "--ro-bind",
        "/",
        "/",
        "--",
        "/bin/true",
    ];
    match run(&namespaces) {
        Ok(o) if o.status.success() => None,
        Ok(o) => Some(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Some(format!("cannot run bwrap: {e}")),
    }
}
