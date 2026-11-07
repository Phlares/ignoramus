//! A registered study in a temp home, shared by the run tests.
#![allow(dead_code)]
use ignorarium::cli::register;
use ignorarium::derived;
use ignorarium::home::Home;
use ignorarium::rules::BoxRecords;
use ignorarium::spec::model::{Direction, Outcome, Paired, SelectionRule};
use ignorarium::{keys, records, spec, stream};
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
    work_of(&lab.home, id)
}

/// `work` for the box at `home`, a lab's `home` folder.
pub fn work_of(home: &Home, id: &str) -> PathBuf {
    let lab = home.root.parent().expect("a lab's home has a parent");
    lab.join(format!("work-{id}"))
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

/// A payload line that copies its own `/proc/self/limits` to `limits` in its output folder.
pub const LIMITS: &str = "cat /proc/self/limits > \"$IGNORARIUM_OUT/limits\"";

/// A payload line whose shell holds about 100 MB.
pub const HOLD_100_MB: &str = "x=$(head -c 100000000 /dev/zero | tr '\\0' a)";

/// The soft and hard limit in the `Max data size` line of `/proc/self/limits` text.
pub fn data_limit(limits: &str) -> Vec<String> {
    let line = limits.lines().find(|l| l.starts_with("Max data size"));
    let words = line.unwrap_or_default().split_whitespace();
    words.skip(3).take(2).map(String::from).collect()
}

/// The box's own data limit, which levels 0 and 1 pass on to a row that declares no memory.
pub fn own_data_limit() -> Vec<String> {
    let own = data_limit(&std::fs::read_to_string("/proc/self/limits").unwrap());
    assert_eq!(own.len(), 2, "{own:?}");
    own
}

/// A payload failure of the payload's own: not the wall timeout, nor the SIGKILL that the
/// out-of-memory killer sends.
pub fn failed_by_itself(exit: &ignorarium::backend::Exit) -> bool {
    use ignorarium::backend::{Exit, TIMEOUT_CODE};
    matches!(exit, Exit::Payload(c) if *c != TIMEOUT_CODE && *c != 128 + 9)
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

/// The bytes of the model input `instrument` names.
pub const MODEL: &[u8] = b"weights v1";

/// The instrument `finished` names: the payload of its register record, an image
/// environment and one model input, `model/m.bin` holding `MODEL`.
pub fn instrument() -> derived::Instrument {
    derived::Instrument {
        payload: "cd".repeat(32),
        environment: "ef".repeat(32),
        inputs: vec![(
            "model/m.bin".into(),
            "model".into(),
            ignorarium::canon::sha256_hex(MODEL),
        )],
    }
}

/// The clusters per stratum, and units per cluster, of a study `finished` builds.
const SOURCE_CLUSTERS: usize = 4;
const SOURCE_UNITS: usize = 5;

/// The strata a study with `estimates` measured: each the estimates name, in order of first
/// appearance; `fiction` when they name none.
pub fn source_strata<'a>(
    estimates: impl IntoIterator<Item = &'a serde_json::Value>,
) -> Vec<String> {
    let mut strata: Vec<String> = Vec::new();
    for e in estimates {
        let s = e["stratum"].as_str().unwrap_or_default();
        if !s.is_empty() && !strata.iter().any(|x| x == s) {
            strata.push(s.into());
        }
    }
    if strata.is_empty() {
        strata.push("fiction".into());
    }
    strata
}

/// The confirmation inputs of a study that measured `strata`: `SOURCE_CLUSTERS` per stratum.
fn source_inputs(strata: &[String]) -> Vec<(String, String)> {
    (strata.iter())
        .flat_map(|s| (0..SOURCE_CLUSTERS).map(move |i| (format!("src/{s}-{i}.txt"), s.clone())))
        .collect()
}

/// The plan text a study that measured `strata` stored: the template, whose hypothesis `H1`
/// tests `arm-a` on `ppl-ratio`, taking `SOURCE_UNITS` units per cluster, with
/// `SOURCE_CLUSTERS` confirmation inputs of that many units in each stratum.
pub fn source_spec(strata: &[String]) -> String {
    let template = include_str!("../../../../templates/numeric-ablation.toml");
    let blocks: String = (source_inputs(strata).iter())
        .map(|(path, stratum)| {
            format!(
                "[[inputs]]\npath = \"{path}\"\nsha256 = \"\"\nrole = \"confirmation\"\nstratum = \"{stratum}\"\nunits = {SOURCE_UNITS}\n[inputs.licence]\nspdx = \"\"\ntype = \"unspecified\"\nsource = \"\"\ncreators = []\n\n"
            )
        })
        .collect();
    template
        .replace(
            "units_per_cluster = 1 ",
            &format!("units_per_cluster = {SOURCE_UNITS} "),
        )
        .replacen("[pins]", &format!("{blocks}[pins]"), 1)
}

/// The `inputs` record and the rows of a study that measured `strata`: on every unit of each
/// input, `exact` and `arm-a` both write 10, so `arm-a` pairs to 0. The template's own two
/// inputs were absent.
fn source_rows(strata: &[String]) -> Vec<(&'static str, serde_json::Value)> {
    let inputs = source_inputs(strata);
    let exact = |path: &str| ignorarium::canon::sha256_hex(path.as_bytes());
    let data: Vec<serde_json::Value> = (inputs.iter())
        .map(|(p, _)| serde_json::json!([exact(p), format!("n-{p}")]))
        .collect();
    let mut frozen = vec![
        serde_json::json!(["data/example.txt", null]),
        serde_json::json!(["data/confirm.txt", null]),
    ];
    frozen.extend(inputs.iter().map(|(p, _)| serde_json::json!([p, exact(p)])));
    let mut out = vec![("inputs", serde_json::json!({ "inputs": frozen }))];
    for (path, _) in &inputs {
        for k in 0..SOURCE_UNITS {
            for condition in ["exact", "arm-a"] {
                let row = serde_json::json!({
                    "stage": "confirmation", "condition": condition, "unit": format!("{path}#{k}"),
                    "repeat": 0, "exit": "ok", "values": { "ppl": 10.0 }, "data_hashes": data,
                });
                out.push(("row", row));
            }
        }
    }
    out
}

/// `finished_with` the common `instrument` and no recheck.
pub fn finished(estimates: Vec<serde_json::Value>) -> Vec<(&'static str, serde_json::Value)> {
    finished_with(instrument(), None, estimates)
}

/// The records of a finished study, as (kind, body): its register record, an analysis record
/// that names `instrument` and `recheck` and promises `estimates`, and the estimates.
pub fn finished_with(
    instrument: derived::Instrument,
    recheck: Option<derived::Recheck>,
    mut estimates: Vec<serde_json::Value>,
) -> Vec<(&'static str, serde_json::Value)> {
    let strata = source_strata(&estimates);
    // The pooled group holds the clusters of every stratum.
    for e in estimates.iter_mut().filter(|e| e["stratum"] == "") {
        e["clusters"] = serde_json::json!(SOURCE_CLUSTERS * strata.len());
        e["units"] = serde_json::json!(SOURCE_CLUSTERS * SOURCE_UNITS * strata.len());
    }
    let register = derived::Register {
        spec_hash: spec::parse(source_spec(&strata).as_bytes()).unwrap().hash,
        leaf: 0,
        log: "testlog".into(),
        defaults: serde_json::json!([]),
        root_id: "12".repeat(32),
        payload: "cd".repeat(32),
        inputs: vec![],
        resolved: vec![],
        deviations: vec![],
        carries: None,
        family: None,
        alpha: None,
        timestamp: None,
    };
    let analysis = derived::Analysis {
        commit: "ef".repeat(32),
        box_version: "0.1.0".into(),
        estimates: estimates.len() as u64,
        verdicts: 0,
        slot: None,
        instrument,
        recheck,
        resize_sd: None,
        appended_sd: None,
    };
    let head = [
        ("register", serde_json::to_value(register).unwrap()),
        ("analysis", serde_json::to_value(analysis).unwrap()),
    ];
    // The rows come last, so the other records keep their places.
    head.into_iter()
        .chain(estimates.into_iter().map(|e| ("estimate", e)))
        .chain(source_rows(&strata))
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

    /// The `source_spec` of each study it holds, from the strata its estimates name.
    fn spec(&self, id: &str) -> Result<Option<Vec<u8>>, String> {
        let Some((_, found)) = self.0.iter().find(|(s, _)| *s == id) else {
            return Ok(None);
        };
        let bodies: Vec<&serde_json::Value> = (found.iter().flatten())
            .filter(|r| r.kind == "estimate")
            .map(|r| &r.body)
            .collect();
        Ok(Some(source_spec(&source_strata(bodies)).into_bytes()))
    }

    /// Each stream is empty.
    fn stream_next(&self, log: &str) -> Result<stream::Next, String> {
        let log = stream::LogName::parse(log).map_err(|e| e.to_string())?;
        stream::next_of(log, &[], &[]).map_err(|e| e.to_string())
    }

    fn stream_closed(&self, _: &str, _: &str, _: u64) -> Result<bool, String> {
        Ok(false)
    }

    /// No study is locked in a family.
    fn locked_members(&self) -> Result<Vec<String>, String> {
        Ok(vec![])
    }

    fn unopened(&self) -> Result<Vec<String>, String> {
        Ok(vec![])
    }
}

/// `Studies` whose stream of `log` gives `next`; another log cannot be read.
pub struct Streamed {
    pub studies: Studies,
    pub log: &'static str,
    pub next: Result<stream::Next, String>,
}

impl BoxRecords for Streamed {
    fn study(&self, id: &str) -> Result<Option<Vec<records::Record>>, String> {
        self.studies.study(id)
    }

    fn spec(&self, id: &str) -> Result<Option<Vec<u8>>, String> {
        self.studies.spec(id)
    }

    fn stream_next(&self, log: &str) -> Result<stream::Next, String> {
        if log == self.log {
            self.next.clone()
        } else {
            Err(format!("no {log} stream here"))
        }
    }

    fn stream_closed(&self, log: &str, study: &str, slot: u64) -> Result<bool, String> {
        self.studies.stream_closed(log, study, slot)
    }

    fn locked_members(&self) -> Result<Vec<String>, String> {
        self.studies.locked_members()
    }

    fn unopened(&self) -> Result<Vec<String>, String> {
        self.studies.unopened()
    }
}

/// The slot and level the stream of `log` in the box at `home` would give next, from its
/// records alone: no register record is checked against it, as `HomeRecords::stream_next`
/// does.
pub fn next_slot(
    home: &Home,
    log: stream::LogName,
    key: &ed25519_dalek::VerifyingKey,
) -> Result<stream::Next, stream::StreamError> {
    let all = records::read_all(stream::Log::of(home, log).path(), key)?;
    stream::next_of(log, &all, &[])
}

/// Sign `bodies` into the record log of study `id` in the box at `home`, and store
/// `source_spec` for the strata of its estimates as its plan unless it has one. Returns the
/// hash of each record.
pub fn plant(home: &Home, id: &str, bodies: &[(&str, serde_json::Value)]) -> Vec<String> {
    let key = keys::load_or_create(home).unwrap();
    std::fs::create_dir_all(home.study_dir(id)).unwrap();
    let plan = home.study_dir(id).join("spec.toml");
    if !plan.exists() {
        let estimates = (bodies.iter())
            .filter(|(kind, _)| *kind == "estimate")
            .map(|(_, body)| body);
        std::fs::write(&plan, source_spec(&source_strata(estimates))).unwrap();
    }
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
    let bodies = finished(estimates);
    let hashes = plant(home, "my-pilot", &bodies);
    (bodies.iter().zip(hashes))
        .filter(|((kind, _), _)| *kind == "estimate")
        .map(|(_, h)| h)
        .collect()
}

/// The template text as a confirmatory plan with its example claim, power_at, estimates,
/// alpha share and deviation uncommented: below 0 by the median, with the sd and icc of
/// `my-pilot`, whose code it does not pin, at the whole level. It is sized at -20, where every
/// unit lies below the margin, and 14 more confirmation inputs give 15 clusters. Each scores 1,
/// so the median test needs 11: 11 clusters give 2.5 x 1.75^8 = 220, past 1/0.005.
pub fn confirmatory(t: String) -> String {
    [
        "power_at = ",
        "estimates = ",
        "[[pins.deviations]]",
        "from = ",
        "reason = ",
        "timing = ",
    ]
    .iter()
    .fold(with_claim(t), |t, k| t.replace(&format!("# {k}"), k))
    .replace("power_at = -1.0", "power_at = -20.0")
    .replacen("[pins]", &format!("{}[pins]", clusters("fiction", 14)), 1)
    .replace("mode = \"exploratory\"", "mode = \"confirmatory\"")
    .replace("label = \"signal\"", "label = \"confirmed\"")
}

/// The text of reserve input `i` of `resize_plan`, which its pin names: its path and a newline.
pub fn reserve_file(i: usize) -> (String, String) {
    let path = format!("data/reserve-{i:02}.txt");
    let text = format!("{path}\n");
    (path, text)
}

/// `confirmatory` as a plan that re-sizes once from `reserve` reserve inputs
/// (`reserve_file`), each one unit in `fiction`. Its 15 confirmation clusters give one unit
/// each; the claim's statistic is a mean in [-2, 2], sized at -1 with the pooled estimate of
/// `resize_pilot`. A row costs 1 s in the estimate.
pub fn resize_plan(reserve: usize) -> impl Fn(String) -> String {
    move |t| {
        let inputs: String = (0..reserve)
            .map(|i| {
                let (path, text) = reserve_file(i);
                let pin = ignorarium::canon::sha256_hex(text.as_bytes());
                listed(&path, "reserve", "fiction", &pin)
            })
            .collect();
        confirmatory(t)
            .replace("{ quantile = 0.5 }", "{ mean = [-2.0, 2.0] }")
            .replace("power_at = -20.0", "power_at = -1.0")
            .replace("units_per_cluster = 3", "units_per_cluster = 1")
            .replace("row_seconds = 60.0", "row_seconds = 1.0")
            .replacen("# [design.resize]", "[design.resize]", 1)
            .replacen("# rule = \"blinded-once\"", "rule = \"blinded-once\"", 1)
            .replacen("[pins]", &format!("{inputs}[pins]"), 1)
    }
}

/// The finished `my-pilot` that `resize_plan` takes its estimate from: the pooled `arm-a`
/// contrast with sd 0.01 and icc 0.1, so the re-size reads a spread near |power_at| = 1.
pub fn resize_pilot(home: &Home) {
    let arm_a = estimate(
        &[("arm-a", 1.0, serde_json::json!({}))],
        "",
        0.0001,
        Some(0.1),
    );
    pilot(home, vec![arm_a]);
}

/// `resize_plan`'s files at registration: every reserve input.
pub fn reserve_files(reserve: usize) -> Vec<(String, String)> {
    (0..reserve).map(reserve_file).collect()
}

/// One input of 3 units, as spec text.
fn listed(path: &str, role: &str, stratum: &str, sha256: &str) -> String {
    format!(
        "[[inputs]]
path = \"{path}\"
sha256 = \"{sha256}\"
role = \"{role}\"
stratum = \"{stratum}\"
units = 3
[inputs.licence]
spdx = \"\"
type = \"unspecified\"
source = \"\"
creators = []

"
    )
}

/// `n` confirmation inputs `data/<stratum>-<i>.txt` of 3 units in `stratum`, as spec text.
pub fn clusters(stratum: &str, n: usize) -> String {
    (0..n)
        .map(|i| {
            listed(
                &format!("data/{stratum}-{i:02}.txt"),
                "confirmation",
                stratum,
                "",
            )
        })
        .collect()
}
