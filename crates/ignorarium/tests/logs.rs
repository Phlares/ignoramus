mod common;

use ed25519_dalek::VerifyingKey;
use ignorarium::derived::{
    self, Analysis, ChildEstimate, DoneRecord, Estimate, Inputs, Register, Resize, Selection,
    SessionEnd, SessionStart, SpawnStop, Stamp, Trigger, Verdict,
};
use ignorarium::records::{self, Record};
use ignorarium::telemetry::RowView;
use std::path::{Path, PathBuf};

/// Each `format-<n>` folder under `tests/fixtures/logs/`, with its number.
fn stored_logs() -> Vec<(u32, PathBuf)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/logs");
    (std::fs::read_dir(root).unwrap())
        .map(|e| e.unwrap().path())
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            let n = name.strip_prefix("format-").and_then(|n| n.parse().ok());
            (
                n.unwrap_or_else(|| panic!("{name}: not a format-<n> folder")),
                p,
            )
        })
        .collect()
}

fn public_key(path: &Path) -> VerifyingKey {
    let text = std::fs::read_to_string(path).unwrap();
    let raw: [u8; 32] = hex::decode(text.trim()).unwrap().try_into().unwrap();
    VerifyingKey::from_bytes(&raw).unwrap()
}

/// Whether the typed reader of `r`'s kind accepts it; `None` for a kind with no typed reader.
/// The stored logs hold no such kind.
fn typed_reads(r: &Record) -> Option<bool> {
    Some(match r.kind.as_str() {
        derived::ANALYSIS => Analysis::of(r).is_some(),
        derived::CHILD_ESTIMATE => ChildEstimate::of(r).is_some(),
        derived::DONE => DoneRecord::of(r).is_some(),
        derived::ESTIMATE => Estimate::of(r).is_some(),
        derived::INPUTS => Inputs::of(r).is_some(),
        derived::REGISTER => Register::of(r).is_some(),
        derived::RESIZE => Resize::of(r).is_some(),
        derived::SELECTION => Selection::of(r).is_some(),
        derived::SESSION_END => SessionEnd::of(r).is_some(),
        derived::SESSION_START => SessionStart::of(r).is_some(),
        derived::SPAWN_STOP => SpawnStop::of(r).is_some(),
        derived::STAMP => Stamp::of(r).is_some(),
        derived::TRIGGER => Trigger::of(r).is_some(),
        derived::VERDICT => Verdict::of(r).is_some(),
        "row" => RowView::of(r).is_some(),
        _ => return None,
    })
}

/// Each folder holds one finished study, written by the box of its format: the record log
/// and the public key of the box that signed it. A later box must read every one.
///
/// Each log is the study of `a_small_study_runs_to_done`, kept with `IGNORARIUM_KEEP_LOG`.
/// `format-0` was written by that test on the code of commit `6fc3f9d`, which has no format
/// number. `format-1` was written by the change that added the number.
///
/// A folder is never edited: a signed log cannot be made again. A new format adds a folder.
#[test]
fn every_stored_log_reads() {
    let logs = stored_logs();
    for n in 0..=records::FORMAT {
        assert!(
            logs.iter().any(|(stored, _)| *stored == n),
            "no stored log of format {n}"
        );
    }
    for (n, dir) in logs {
        let (key, log) = (dir.join("public-key.hex"), dir.join("records.jsonl"));
        // The public-key check passes with no box home.
        let home = tempfile::tempdir().unwrap();
        assert_cmd::Command::cargo_bin("ignorarium")
            .unwrap()
            .env_remove("IGNORARIUM_HOME")
            .env("HOME", home.path())
            .args(["verify", "--public-key"])
            .arg(&key)
            .arg("--records")
            .arg(&log)
            .assert()
            .code(0);
        // Every kind is known, and each typed reader accepts its records.
        let all = records::read_all(&log, &public_key(&key)).unwrap();
        assert_eq!(all[0].kind, derived::REGISTER, "format {n}");
        assert_eq!(records::format_of(&all[0]).unwrap(), n, "format {n}");
        assert!(
            all.iter().any(|r| r.kind == derived::DONE),
            "format {n}: not a finished study"
        );
        for r in &all {
            assert_eq!(
                typed_reads(r),
                Some(true),
                "format {n}: record {} ({}) is not read",
                r.seq,
                r.kind
            );
        }
    }
}

/// The smallest study, run from `register` to `done` through the binary.
/// `IGNORARIUM_KEEP_LOG=<folder>` keeps its record log and the box's public key there; that
/// is how each stored log under `tests/fixtures/logs/` is written.
#[test]
fn a_small_study_runs_to_done() {
    let lab = common::lab();
    common::plain(&lab, "small");
    common::bin(&lab)
        .current_dir(common::work(&lab, "small"))
        .args(["run", "small"])
        .assert()
        .code(0);
    assert_eq!(common::kind_of(&lab, "small", derived::DONE).len(), 1);
    if let Some(keep) = std::env::var_os("IGNORARIUM_KEEP_LOG") {
        let keep = PathBuf::from(keep);
        std::fs::create_dir_all(&keep).unwrap();
        let log = lab.home.study_dir("small").join("records.jsonl");
        std::fs::copy(log, keep.join("records.jsonl")).unwrap();
        let key = common::bin(&lab).args(["key", "export"]).assert().code(0);
        std::fs::write(keep.join("public-key.hex"), common::text(&key)).unwrap();
    }
}
