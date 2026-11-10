use ignorarium::backend::level0::Level0;
use ignorarium::cli::register;
use ignorarium::derived::{Register, Resize, Verdict};
use ignorarium::home::Home;
use ignorarium::planlog::store::PlanLog;
use ignorarium::runner::{RunStatus, prepare_with, run};
use ignorarium::{keys, prior, records, spec};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;
use std::process::Command;

fn litmus() -> Command {
    Command::new(env!("CARGO_BIN_EXE_litmus"))
}

/// Register the plan at `spec_path` with its approval typed, then run it at level 0.
fn register_and_run(home: &Home, root: &Path, spec_path: &Path, id: &str) {
    let hash = spec::load(spec_path).unwrap().hash;
    let typed = format!("{}\n", &hash["sha256:".len()..][..8]);
    let mut out = Vec::new();
    let done = register::run(spec_path, home, &mut Cursor::new(typed), &mut out, true)
        .unwrap_or_else(|e| panic!("{}: {}", e.msg, String::from_utf8_lossy(&out)));
    assert_eq!(done.code, 0, "{id}: {}", String::from_utf8_lossy(&out));
    let plan = prepare_with(home, id, root, vec![Box::new(Level0)]).unwrap();
    assert_eq!(run(&plan).unwrap(), RunStatus::Done, "{id}");
}

fn records_of(home: &Home, id: &str, kind: &str) -> Vec<records::Record> {
    let key = keys::load(home).unwrap().unwrap().verifying_key();
    let all = records::read_all(&home.study_dir(id).join("records.jsonl"), &key).unwrap();
    all.into_iter().filter(|r| r.kind == kind).collect()
}

#[test]
fn litmus_tells_true_from_false() {
    let tmp = tempfile::tempdir().unwrap();
    let study = tmp.path().join("study");
    let made = (litmus().args(["gen", "--seed", "e2e", "--out"]).arg(&study))
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    let home = Home::open_at(&tmp.path().join("home")).unwrap();
    let pilot = study.join("spec/pilot.toml");
    register_and_run(&home, &study, &pilot, "litmus-pilot-e2e");
    register_and_run(&home, &study, &study.join("spec/litmus.toml"), "litmus-e2e");

    let verdicts: BTreeMap<String, Verdict> = (records_of(&home, "litmus-e2e", "verdict").iter())
        .map(|r| Verdict::of(r).unwrap())
        .map(|v| (v.hypothesis.clone(), v))
        .collect();
    for v in verdicts.values() {
        println!(
            "{}: {} e = {:?} level = {:?} reason = {:?}",
            v.hypothesis, v.verdict, v.e_value, v.level, v.reason
        );
    }
    let said = |h: &str| verdicts[h].verdict.as_str();
    assert_eq!(said("good"), "pass");
    assert_eq!(said("bad"), "fail");
    assert_eq!(said("edge"), "fail");

    let resized = records_of(&home, "litmus-e2e", "resize");
    assert_eq!(resized.len(), 1);
    let resize = Resize::of(&resized[0]).unwrap();
    println!("resize: {resize:?}");
    let [group] = &resize.strata[..] else {
        panic!("one pooled group: {resize:?}")
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
    let register = Register::of(&records_of(&home, "litmus-e2e", "register")[0]).unwrap();
    assert_eq!(register.log, "testlog");
    let leaves = (PlanLog::open(&home.root.join(&register.log)).unwrap())
        .leaves()
        .unwrap();
    let released: Vec<_> = (leaves.iter())
        .filter(|l| l.kind == "release")
        .flat_map(|l| &l.data_hashes)
        .collect();
    assert!(released.iter().all(|d| reserve.contains(d)), "{released:?}");
    assert!(!released.is_empty());
    assert_eq!(released.len() as u64, group.reserve - group.appended);

    let entries: Vec<serde_json::Value> = (verdicts.values())
        .map(|v| serde_json::json!({ "hypothesis": v.hypothesis, "verdict": v.verdict }))
        .collect();
    let report = tmp.path().join("report.json");
    std::fs::write(
        &report,
        serde_json::json!({ "hypotheses": entries }).to_string(),
    )
    .unwrap();
    let checked = (litmus().arg("verify").arg("--report").arg(&report))
        .arg("--expected")
        .arg(study.join("expected.json"))
        .output()
        .unwrap();
    assert!(checked.status.success(), "{checked:?}");
}
