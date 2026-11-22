use ignorarium::backend::level0::Level0;
use ignorarium::cli::register;
use ignorarium::derived::{Register, Resize, ResizeStratum, Verdict};
use ignorarium::home::Home;
use ignorarium::planlog::store::PlanLog;
use ignorarium::runner::{RunStatus, prepare_with, run};
use ignorarium::{keys, prior, records, spec};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn litmus() -> Command {
    Command::new(env!("CARGO_BIN_EXE_litmus"))
}

/// Register the plan at `spec_path` with its approval typed, then run it at level 0.
/// `Err` holds what registration printed when it refused the plan.
fn register_and_run(home: &Home, root: &Path, spec_path: &Path, id: &str) -> Result<(), String> {
    let hash = spec::load(spec_path).unwrap().hash;
    let typed = format!("{}\n", &hash["sha256:".len()..][..8]);
    let mut out = Vec::new();
    let done = register::run(spec_path, home, &mut Cursor::new(typed), &mut out, true);
    let said = String::from_utf8_lossy(&out);
    match done {
        Err(e) => return Err(format!("{id}: {}: {said}", e.msg)),
        Ok(d) if d.code != 0 => return Err(format!("{id}: {said}")),
        Ok(_) => {}
    }
    let plan = prepare_with(home, id, root, vec![Box::new(Level0)])
        .unwrap_or_else(|e| panic!("{id}: cannot prepare: {e}"));
    let status = run(&plan).unwrap_or_else(|e| panic!("{id}: the run failed: {e}"));
    assert_eq!(status, RunStatus::Done, "{id}: the run ended {status:?}");
    Ok(())
}

fn records_of(home: &Home, id: &str, kind: &str) -> Vec<records::Record> {
    let key = keys::load(home).unwrap().unwrap().verifying_key();
    let all = records::read_all(&home.study_dir(id).join("records.jsonl"), &key).unwrap();
    all.into_iter().filter(|r| r.kind == kind).collect()
}

/// One finished litmus: its verdicts by hypothesis, the re-size of its one pooled group, and
/// the reserve inputs its release leaves name. `_tmp` holds the study folder and the box home.
struct Litmus {
    _tmp: TempDir,
    id: String,
    study: PathBuf,
    verdicts: BTreeMap<String, Verdict>,
    resize: ResizeStratum,
    released: u64,
}

impl Litmus {
    fn verdict(&self, h: &str) -> &str {
        let v = self.verdicts.get(h);
        let v = v.unwrap_or_else(|| panic!("{}: no verdict for {h}", self.id));
        &v.verdict
    }
}

/// Generate the litmus of `seed` with `--environment environment` in a new box home, then
/// register and run its pilot and its study at level 0. `Err` holds what registration printed
/// when it refused a plan.
fn litmus_run(seed: &str, environment: &str) -> Result<Litmus, String> {
    let tmp = tempfile::tempdir().unwrap();
    let study = tmp.path().join("study");
    let made = (litmus().args(["gen", "--seed", seed, "--environment", environment]))
        .arg("--out")
        .arg(&study)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    let home = Home::open_at(&tmp.path().join("home")).unwrap();
    let (pilot, id) = (format!("litmus-pilot-{seed}"), format!("litmus-{seed}"));
    register_and_run(&home, &study, &study.join("spec/pilot.toml"), &pilot)?;
    register_and_run(&home, &study, &study.join("spec/litmus.toml"), &id)?;

    let verdicts = (records_of(&home, &id, "verdict").iter())
        .map(|r| Verdict::of(r).unwrap())
        .map(|v| (v.hypothesis.clone(), v))
        .collect();
    let resized = records_of(&home, &id, "resize");
    assert_eq!(resized.len(), 1, "{id}: resize records");
    let resize = Resize::of(&resized[0]).unwrap();
    let [group] = &resize.strata[..] else {
        panic!("{id}: not one pooled group: {resize:?}")
    };

    // The release names each reserve input the re-size did not append, and nothing else.
    let reserve: Vec<(String, String)> = (std::fs::read_dir(study.join("data")).unwrap())
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("reserve-")
        })
        .map(|p| prior::file_hashes(&std::fs::read(p).unwrap()))
        .collect();
    let register = Register::of(&records_of(&home, &id, "register")[0]).unwrap();
    assert_eq!(register.log, "testlog");
    let leaves = (PlanLog::open(&home.root.join(&register.log)).unwrap())
        .leaves()
        .unwrap();
    let released: Vec<_> = (leaves.iter())
        .filter(|l| l.kind == "release")
        .flat_map(|l| &l.data_hashes)
        .collect();
    assert!(released.iter().all(|d| reserve.contains(d)), "{released:?}");
    assert_eq!(
        released.len() as u64,
        group.reserve - group.appended,
        "{id}"
    );

    Ok(Litmus {
        released: released.len() as u64,
        resize: group.clone(),
        _tmp: tmp,
        id,
        study,
        verdicts,
    })
}

#[test]
fn litmus_tells_true_from_false() {
    let r = litmus_run("e2e", "").unwrap_or_else(|why| panic!("refused: {why}"));
    for v in r.verdicts.values() {
        println!(
            "{}: {} e = {:?} level = {:?} reason = {:?}",
            v.hypothesis, v.verdict, v.e_value, v.level, v.reason
        );
    }
    println!("resize: {:?}", r.resize);
    assert_eq!(r.verdict("good"), "pass");
    assert_eq!(r.verdict("bad"), "fail");
    assert_eq!(r.verdict("edge"), "fail");
    assert!(r.released > 0, "no reserve input released");

    // The report is built from the verdict records, not by `ignorarium report`: this ties
    // `expected.json` to the box's verdict strings and `verify` to the report's shape.
    let entries: Vec<serde_json::Value> = (r.verdicts.values())
        .map(|v| serde_json::json!({ "hypothesis": v.hypothesis, "verdict": v.verdict }))
        .collect();
    let report = r.study.join("report.json");
    std::fs::write(
        &report,
        serde_json::json!({ "hypotheses": entries }).to_string(),
    )
    .unwrap();
    let checked = (litmus().arg("verify").arg("--report").arg(&report))
        .arg("--expected")
        .arg(r.study.join("expected.json"))
        .output()
        .unwrap();
    assert!(checked.status.success(), "{checked:?}");
}

/// A level 0 run records environment `host`, which no pin matches, so the study registers only
/// through its deviation from the pilot. The pinned image need not exist at level 0.
#[test]
fn pinned_environment_runs_at_level_0() {
    let r = litmus_run("e2e", &"ab".repeat(32)).unwrap_or_else(|why| panic!("refused: {why}"));
    assert_eq!(r.verdict("good"), "pass");
    assert_eq!(r.verdict("bad"), "fail");
    assert_eq!(r.verdict("edge"), "fail");
}

/// The least, median and greatest of `v`.
fn spread(mut v: Vec<u64>) -> (u64, u64, u64) {
    v.sort_unstable();
    (v[0], v[v.len() / 2], v[v.len() - 1])
}

/// A seed, its re-size, and its verdict by hypothesis.
type Swept = (String, ResizeStratum, BTreeMap<String, String>);

/// The seeds of `runs` that `keep` picks.
fn seeds(runs: &[Swept], keep: impl Fn(&Swept) -> bool) -> Vec<&str> {
    runs.iter()
        .filter(|r| keep(r))
        .map(|r| r.0.as_str())
        .collect()
}

#[test]
#[ignore = "runs the litmus once per seed; LITMUS_SWEEP sets the count, 20 by default"]
fn seeds_sweep() {
    let n: usize = std::env::var("LITMUS_SWEEP").map_or(20, |v| v.parse().unwrap());
    assert!(
        n >= 1,
        "LITMUS_SWEEP must be at least 1, so the sweep runs a seed"
    );
    let mut refused = Vec::new();
    let mut runs: Vec<Swept> = Vec::new();
    for i in 0..n {
        let seed = format!("sweep-{i}");
        let r = match litmus_run(&seed, "") {
            Ok(r) => r,
            Err(why) => {
                println!("{seed}: refused: {why}");
                refused.push(seed);
                continue;
            }
        };
        let said = (r.verdicts.values())
            .map(|v| format!("{} {} e={:?}", v.hypothesis, v.verdict, v.e_value))
            .collect::<Vec<_>>()
            .join(", ");
        let g = &r.resize;
        let (needed, appended) = (g.needed, g.appended);
        println!(
            "{seed}: needed {needed:?} appended {appended} released {} | {said}",
            r.released
        );
        let verdicts = ["good", "bad", "edge"].map(|h| (h.to_string(), r.verdict(h).to_string()));
        runs.push((seed, r.resize.clone(), verdicts.into()));
    }

    println!("accepted {} of {n}", runs.len());
    let needed: Vec<u64> = runs.iter().filter_map(|r| r.1.needed).collect();
    let appended: Vec<u64> = runs.iter().map(|r| r.1.appended).collect();
    if !needed.is_empty() {
        println!("needed min/median/max {:?}", spread(needed));
        println!("appended min/median/max {:?}", spread(appended));
    }
    let mut counts: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for (h, v) in runs.iter().flat_map(|r| &r.2) {
        *counts.entry((h, v)).or_default() += 1;
    }
    println!("verdicts {counts:?}");
    let edge_passes = seeds(&runs, |r| r.2["edge"] == "pass");
    println!("edge passes: {edge_passes:?}");

    assert!(refused.is_empty(), "refused: {refused:?}");
    let good_not = seeds(&runs, |r| r.2["good"] != "pass");
    assert!(good_not.is_empty(), "good did not pass: {good_not:?}");
    let bad_not = seeds(&runs, |r| r.2["bad"] != "fail");
    assert!(bad_not.is_empty(), "bad did not fail: {bad_not:?}");
    let short = seeds(&runs, |r| {
        !(r.1.needed).is_some_and(|k| k <= r.1.first + r.1.reserve)
    });
    assert!(short.is_empty(), "needed past first + reserve: {short:?}");
    // On the margin a pass is a false positive of the test, at its level per seed.
    assert!(edge_passes.len() <= 1, "edge passes: {edge_passes:?}");
}
