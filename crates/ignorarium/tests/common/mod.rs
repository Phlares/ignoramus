//! A registered study in a temp home, shared by the run tests.
#![allow(dead_code)]
use ignorarium::cli::register;
use ignorarium::derived;
use ignorarium::home::Home;
use ignorarium::rules::BoxRecords;
use ignorarium::spec::model::{Direction, Outcome, Paired, SelectionRule};
use ignorarium::{keys, records, spec};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The shell line that writes the template's one measure.
pub const VALUES: &str = "printf '{\"ppl\": 1.5}' > \"$IGNORARIUM_OUT/values.json\"";

pub const PAYLOAD: &str = "#!/bin/sh\nprintf '%s %s %s' \"$2\" \"$4\" \"$6\" > \"$IGNORARIUM_OUT/result\"\nprintf '{\"ppl\": 1.5}' > \"$IGNORARIUM_OUT/values.json\"\n";

/// The template text with its example claim, statistic and alpha share uncommented: below 0
/// by the median, with the whole level.
pub fn with_claim(t: String) -> String {
    ["claim", "statistic", "alpha_share"]
        .iter()
        .fold(t, |t, k| {
            t.replace(&format!("# {k} = "), &format!("{k} = "))
        })
}

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

impl Drop for Lab {
    fn drop(&mut self) {
        unseal(&self.root);
    }
}

/// Give the owner write access to `dir` and every folder below it again, so a test can
/// change a read-only copy and its temp folder can be removed. Links are not followed.
pub fn unseal(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(m) = dir.symlink_metadata() else {
        return;
    };
    if !m.is_dir() {
        return;
    }
    let mode = m.permissions().mode() | 0o700;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode));
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        unseal(&e.path());
    }
}

/// The study root that `register_study` registers `id` from.
pub fn work(lab: &Lab, id: &str) -> PathBuf {
    lab.root.join(format!("work-{id}"))
}

/// Register a study with `register::run` from its own root, which holds `payload/run` and
/// the files in `before` (paths relative to the root). Returns the study folder in the box.
pub fn register_study(
    lab: &Lab,
    id: &str,
    edit: impl Fn(String) -> String,
    before: &[(&str, &str)],
    payload: &str,
) -> PathBuf {
    let work = work(lab, id);
    std::fs::create_dir_all(work.join("data")).unwrap();
    std::fs::create_dir_all(work.join("payload")).unwrap();
    write_exec(&work.join("payload/run"), payload);
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
    lab.home.study_dir(id)
}

/// A study with the template's inputs: `data/example.txt`, written, and the confirmation
/// input `data/confirm.txt`, absent, whose 3 declared units make 3 units x 2 conditions = 6 rows.
pub fn plain(lab: &Lab, id: &str) -> PathBuf {
    with_payload(lab, id, PAYLOAD)
}

/// `plain` with another payload.
pub fn with_payload(lab: &Lab, id: &str, payload: &str) -> PathBuf {
    edited(lab, id, |t| t, payload)
}

/// `with_payload` with the spec text changed by `edit` before registration.
pub fn edited(lab: &Lab, id: &str, edit: impl Fn(String) -> String, payload: &str) -> PathBuf {
    register_study(
        lab,
        id,
        edit,
        &[("data/example.txt", "calibration")],
        payload,
    )
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

/// Where binfmt_misc handlers are registered.
pub const BINFMT: &str = "/proc/sys/fs/binfmt_misc";

/// `None` when this host has no WSL interop handler. Otherwise whether level 1 refuses the
/// host: a `WSLInterop` handler is enabled with the `F` flag, or cannot be read.
pub fn interop_refused() -> Option<bool> {
    let names: Vec<String> = std::fs::read_dir(BINFMT)
        .ok()?
        .filter_map(|e| Some(e.ok()?.file_name().to_string_lossy().into_owned()))
        .filter(|n| n.starts_with("WSLInterop"))
        .collect();
    let refused = |name: &String| match std::fs::read_to_string(Path::new(BINFMT).join(name)) {
        Ok(t) => {
            let flags = t.lines().find_map(|l| l.strip_prefix("flags:"));
            t.lines().next() == Some("enabled") && flags.is_some_and(|f| f.contains('F'))
        }
        Err(_) => true,
    };
    (!names.is_empty()).then(|| names.iter().any(refused))
}

/// Why bubblewrap cannot run a level 1 sandbox on this host, or `None` when it can.
/// Found by calling `bwrap` directly, never through the box's code, so a fault in that
/// code fails the level 1 tests instead of skipping them. A WSL interop handler that level 1
/// refuses (see `interop_refused`) makes the host one without level 1, as the probe does.
pub fn no_level1() -> Option<String> {
    if interop_refused() == Some(true) {
        return Some(format!(
            "a WSL interop handler in {BINFMT} has the F flag or cannot be read"
        ));
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

/// A record of study `study` for a box built by hand. `seq` keeps two records with one body
/// apart: each has its own hash.
pub fn record(study: &str, seq: u64, kind: &str, body: serde_json::Value) -> records::Record {
    records::Record {
        seq,
        prev: "0".repeat(64),
        study: study.into(),
        row: None,
        kind: kind.into(),
        body,
        ts: "2026-01-01T00:00:00Z".into(),
    }
}

/// The template's outcome: `ppl-ratio`, measured as `ppl`, paired in percent.
pub fn ppl_ratio() -> Outcome {
    Outcome {
        id: "ppl-ratio".into(),
        unit: "ratio".into(),
        direction: Direction::Lower,
        measure: "ppl".into(),
        paired: Paired::Percent,
        registered: true,
    }
}

/// A term of a contrast: condition id, weight and params.
pub type Term<'a> = (&'a str, f64, serde_json::Value);

/// The body of an estimate of the template outcome for the contrast `terms` against the
/// template's reference `exact`, in `stratum`.
pub fn estimate(terms: &[Term], stratum: &str, var: f64, icc: Option<f64>) -> serde_json::Value {
    estimate_against(("exact", serde_json::json!({})), terms, stratum, var, icc)
}

/// `estimate` against another reference condition.
pub fn estimate_against(
    reference: (&str, serde_json::Value),
    terms: &[Term],
    stratum: &str,
    var: f64,
    icc: Option<f64>,
) -> serde_json::Value {
    let terms: Vec<(String, f64, serde_json::Value)> = terms
        .iter()
        .map(|(c, w, p)| (c.to_string(), *w, p.clone()))
        .collect();
    let reference = (reference.0.to_string(), reference.1);
    let (key, key_hash) = derived::contrast_key(&ppl_ratio(), 1, &reference, &terms);
    serde_json::json!({
        "hypothesis": "H1", "outcome": "ppl-ratio", "key": key, "key_hash": key_hash,
        "stratum": stratum, "clusters": 4, "units": 20, "mean": 1.0, "var": var, "icc": icc
    })
}

/// The pooled estimate of the template's contrast, `arm-a` alone: variance 4, icc 0.1.
pub fn arm_a() -> serde_json::Value {
    estimate(&[("arm-a", 1.0, serde_json::json!({}))], "", 4.0, Some(0.1))
}

/// The records of a finished study, as (kind, body): its register record, an analysis record
/// that promises `estimates`, and the estimates.
pub fn finished(estimates: Vec<serde_json::Value>) -> Vec<(&'static str, serde_json::Value)> {
    let register = derived::Register {
        spec_hash: format!("sha256:{}", "ab".repeat(32)),
        leaf: 0,
        log: "testlog".into(),
        defaults: serde_json::json!([]),
        root: "/work/pilot".into(),
        payload: "cd".repeat(32),
        inputs: vec![],
        resolved: vec![],
        timestamp: None,
    };
    let analysis = derived::Analysis {
        commit: "ef".repeat(32),
        box_version: "0.1.0".into(),
        estimates: estimates.len() as u64,
        verdicts: 0,
        slot: None,
    };
    let head = [
        ("register", serde_json::to_value(register).unwrap()),
        ("analysis", serde_json::to_value(analysis).unwrap()),
    ];
    head.into_iter()
        .chain(estimates.into_iter().map(|e| ("estimate", e)))
        .collect()
}

/// `finished`, with the `selection` record the selection stage writes before the analysis:
/// the pick was `selected`, over the one candidate `arm-a`.
pub fn finished_selecting(
    selected: &str,
    estimates: Vec<serde_json::Value>,
) -> Vec<(&'static str, serde_json::Value)> {
    let selection = derived::Selection {
        metric: "ppl-ratio".into(),
        rule: SelectionRule::BestMean,
        direction: Direction::Lower,
        candidates: vec![derived::Candidate {
            id: "arm-a".into(),
            mean: -1.0,
        }],
        selected: selected.into(),
        units: vec!["data/select.txt#0".into()],
        complete_units: 1,
        incomplete_units: 0,
    };
    let mut all = finished(estimates);
    all.insert(1, ("selection", serde_json::to_value(selection).unwrap()));
    all
}

/// `bodies` as the records of study `id`, in order, for a hand-built box.
pub fn records_for(id: &str, bodies: &[(&str, serde_json::Value)]) -> Vec<records::Record> {
    bodies
        .iter()
        .enumerate()
        .map(|(i, (kind, body))| record(id, i as u64, kind, body.clone()))
        .collect()
}

/// A box built by hand: each study id with its records, or the error its log gives.
pub struct Studies(pub Vec<(&'static str, Result<Vec<records::Record>, String>)>);

impl BoxRecords for Studies {
    fn study(&self, id: &str) -> Result<Option<Vec<records::Record>>, String> {
        match self.0.iter().find(|(s, _)| *s == id) {
            Some((_, found)) => found.clone().map(Some),
            None => Ok(None),
        }
    }
}

/// Sign `bodies` into the record log of study `id` in the box at `home`. Returns the hash of
/// each record.
pub fn plant(home: &Home, id: &str, bodies: &[(&str, serde_json::Value)]) -> Vec<String> {
    let key = keys::load_or_create(home).unwrap();
    std::fs::create_dir_all(home.study_dir(id)).unwrap();
    let path = home.study_dir(id).join("records.jsonl");
    bodies
        .iter()
        .map(|(kind, body)| {
            let draft = records::RecordDraft {
                study: id.into(),
                row: None,
                kind: kind.to_string(),
                body: body.clone(),
            };
            records::hash(&records::append(&path, &key, draft).unwrap())
        })
        .collect()
}

/// A finished study `my-pilot`, the one the template's commented `estimates` names, with
/// `estimates`. Returns the hash of each estimate record.
pub fn pilot(home: &Home, estimates: Vec<serde_json::Value>) -> Vec<String> {
    let n = estimates.len();
    let hashes = plant(home, "my-pilot", &finished(estimates));
    hashes[hashes.len() - n..].to_vec()
}

/// The template text as a confirmatory plan with its example claim, power_at, estimates and
/// alpha share uncommented: below 0 by the median, with the sd and icc of `my-pilot`, at the
/// whole level. It is sized at -20, where every unit lies below the margin, and 14 more
/// confirmation inputs give the 15 clusters the median test needs: each scores 1, and
/// 1.5^14 = 292 reaches 1/0.005.
pub fn confirmatory(t: String) -> String {
    ["power_at", "estimates"]
        .iter()
        .fold(with_claim(t), |t, k| {
            t.replace(&format!("# {k} = "), &format!("{k} = "))
        })
        .replace("power_at = -1.0", "power_at = -20.0")
        .replacen("[pins]", &format!("{}[pins]", clusters("fiction", 14)), 1)
        .replace("mode = \"exploratory\"", "mode = \"confirmatory\"")
        .replace("label = \"signal\"", "label = \"confirmed\"")
}

/// `n` confirmation inputs `data/<stratum>-<i>.txt` of 3 units in `stratum`, as spec text.
pub fn clusters(stratum: &str, n: usize) -> String {
    (0..n)
        .map(|i| {
            format!(
                "[[inputs]]
path = \"data/{stratum}-{i:02}.txt\"
sha256 = \"\"
role = \"confirmation\"
stratum = \"{stratum}\"
units = 3
[inputs.licence]
spdx = \"\"
type = \"unspecified\"
source = \"\"
creators = []

"
            )
        })
        .collect()
}
