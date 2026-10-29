mod common;
use bundle::licence::Kind;
use bundle::manifest::{self, sha256_bytes};
use bundle::profile::{self, Level};
use bundle::{Error, Problem, build, zip, zip_name};
use common::{INPUT, facts, fixture, release};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &dest);
        } else {
            fs::copy(e.path(), dest).unwrap();
        }
    }
}

fn study_copy(tmp: &Path) -> PathBuf {
    let dir = tmp.join("study");
    copy_tree(&fixture(), &dir);
    dir
}

fn refused(r: Result<bundle::Built, Error>) -> (Vec<Problem>, Vec<Problem>) {
    match r {
        Err(Error::Refused { problems, warnings }) => (problems, warnings),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn codes(problems: &[Problem]) -> Vec<&'static str> {
    let mut c: Vec<_> = problems.iter().map(|p| p.code).collect();
    c.sort_unstable();
    c.dedup();
    c
}

fn json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn profiles_follow_the_table() {
    let p = profile::for_label("signal").unwrap();
    assert_eq!(p.name, "exploratory");
    assert_eq!(p.steps.len(), 9);
    let level = |p: &profile::Profile, id: &str| p.steps.iter().find(|s| s.id == id).unwrap().level;
    assert_eq!(level(&p, "archive-doi"), Level::Choice);
    assert_eq!(level(&p, "index-entry"), Level::OptIn);
    let c = profile::for_label("confirmed").unwrap();
    assert_eq!(c.name, "confirmatory");
    assert_eq!(level(&c, "archive-doi"), Level::Required);
    assert_eq!(level(&c, "preprint"), Level::Choice);
    for label in ["replicated", "generalized"] {
        assert_eq!(profile::for_label(label).unwrap().name, "confirmatory");
    }
    let r = profile::named("replay").unwrap();
    assert_eq!(level(&r, "archive-doi"), Level::None);
    assert_eq!(level(&r, "index-entry"), Level::Listed);
    assert_eq!(r.bytes, "No new bytes.");
    let head = r.steps.iter().find(|s| s.id == "plan-log-head").unwrap();
    assert_eq!(head.file.as_deref(), Some("records/planlog-head.json"));
    assert_eq!(
        head.note,
        "the latest signed tree head and a consistency proof to the last public head"
    );
    assert_eq!(r.steps[0].note, "");
    assert!(profile::for_label("bogus").is_none());
    assert!(profile::named("bogus").is_none());
}

#[test]
fn fixture_bundle_builds_and_verifies() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    let built = build(&fixture(), &out, &facts(), &release()).unwrap();
    let mut want: Vec<String> = bundle::GENERATED.iter().map(|s| s.to_string()).collect();
    for p in [
        "LICENSES/Apache-2.0.txt",
        "LICENSES/CC-BY-SA-4.0.txt",
        "LICENSES/CC0-1.0.txt",
        "README.md",
        "citation.toml",
        INPUT,
        "records/planlog-head.json",
        "results/summary.json",
        "spec.toml",
        "src/run",
    ] {
        want.push(p.to_string());
    }
    want.sort();
    let got: Vec<String> = manifest::scan(&out)
        .unwrap()
        .into_iter()
        .map(|e| e.path)
        .collect();
    assert_eq!(got, want);
    assert_eq!(manifest::check(&out).unwrap(), built.manifest_sha256);
    assert_eq!(built.dir, out);
    assert!(built.warnings.is_empty());
    assert!(built.artifacts.is_empty());
    assert_eq!(built.profile.name, "exploratory");

    assert_eq!(
        fs::read_to_string(out.join("CITATION.cff")).unwrap(),
        include_str!("fixtures/expected/CITATION.cff")
    );
    assert_eq!(
        json(&out.join("datacite.json"))["doi"],
        "10.5072/zenodo.123456"
    );
    assert_eq!(
        json(&out.join("publish-profile.json"))["name"],
        "exploratory"
    );
    let crate_doc = json(&out.join("ro-crate-metadata.json"));
    let root = crate_doc["@graph"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["@id"] == "./")
        .unwrap();
    let mut parts: Vec<&str> = root["hasPart"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["@id"].as_str().unwrap())
        .collect();
    parts.sort_unstable();
    let mut want_parts: Vec<&str> = want
        .iter()
        .map(String::as_str)
        .filter(|p| *p != "ro-crate-metadata.json" && *p != "MANIFEST.sha256")
        .collect();
    want_parts.sort_unstable();
    assert_eq!(parts, want_parts);
    let lic = fs::read_to_string(out.join("LICENSE.md")).unwrap();
    assert!(lic.contains("| `data/texts/a.txt` | CC-BY-SA-4.0 |"));
    assert_eq!(
        fs::read_to_string(out.join("ARTIFACTS.sha256")).unwrap(),
        ""
    );
    assert_eq!(
        fs::read(out.join("records/planlog-head.json")).unwrap(),
        b"{\"size\": 1}\n"
    );
}

#[test]
fn same_inputs_give_same_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    let one = build(&fixture(), &a, &facts(), &release()).unwrap();
    let two = build(&fixture(), &b, &facts(), &release()).unwrap();
    assert_eq!(one.manifest_sha256, two.manifest_sha256);
    let (za, zb) = (tmp.path().join("a.zip"), tmp.path().join("b.zip"));
    zip::pack(&a, &za).unwrap();
    zip::pack(&b, &zb).unwrap();
    let (ba, bb) = (fs::read(za).unwrap(), fs::read(zb).unwrap());
    assert!(!ba.is_empty());
    assert_eq!(sha256_bytes(&ba), sha256_bytes(&bb));
}

#[test]
fn every_problem_is_reported_at_once() {
    let tmp = tempfile::tempdir().unwrap();
    let study = study_copy(tmp.path());
    fs::remove_file(study.join("README.md")).unwrap();
    fs::write(study.join("notes.txt"), "x\n").unwrap();
    let cit = fs::read_to_string(study.join("citation.toml")).unwrap();
    let cit = cit.replacen(
        "title = \"Fixture study: lookup tables in place of multiplication\"",
        "title = \" \"",
        1,
    );
    fs::write(study.join("citation.toml"), cit).unwrap();
    fs::remove_file(study.join("LICENSES/CC-BY-SA-4.0.txt")).unwrap();
    let mut f = facts();
    f.licensing.inputs[0].licence.kind = Kind::Unspecified;
    f.runs[0].result = vec![bundle::rocrate::Item::Path("results/missing.json".into())];
    f.attachments.clear();
    f.label = "confirmed".into();
    let out = tmp.path().join("out");
    let (problems, _) = refused(build(&study, &out, &f, &release()));
    assert_eq!(
        codes(&problems),
        [
            "E-PUB-002",
            "E-PUB-003",
            "E-PUB-004",
            "E-PUB-006",
            "E-PUB-007",
            "E-PUB-008",
            "E-PUB-009"
        ]
    );
    let at = |code: &str| {
        problems
            .iter()
            .find(|p| p.code == code)
            .unwrap()
            .path
            .clone()
    };
    assert_eq!(at("E-PUB-002"), "README.md");
    assert_eq!(at("E-PUB-003"), "notes.txt");
    assert_eq!(at("E-PUB-009"), "records/planlog-head.json");
    assert!(!out.exists());
}

#[test]
fn unknown_label_and_bad_release_are_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let mut f = facts();
    f.label = "bogus".into();
    let mut r = release();
    r.date = "soon".into();
    let (problems, _) = refused(build(&fixture(), &tmp.path().join("out"), &f, &r));
    assert_eq!(codes(&problems), ["E-PUB-013", "E-PUB-014"]);
    let p = problems.iter().find(|p| p.code == "E-PUB-014").unwrap();
    assert_eq!(p.path, "label");
    assert_eq!(
        p.instruction,
        "Use one of the claim labels: signal, confirmed, replicated, generalized."
    );
}

#[test]
fn stale_generated_files_are_regenerated() {
    let tmp = tempfile::tempdir().unwrap();
    let study = study_copy(tmp.path());
    fs::write(study.join("CITATION.cff"), "title: \"old\"\n").unwrap();
    fs::create_dir_all(study.join("doc")).unwrap();
    fs::write(study.join("doc/CITATION.cff"), "nested\n").unwrap();
    let out = tmp.path().join("out");
    build(&study, &out, &facts(), &release()).unwrap();
    let cff = fs::read_to_string(out.join("CITATION.cff")).unwrap();
    assert!(!cff.contains("\"old\""));
    assert!(cff.contains("cff-version: \"1.2.0\""));
    assert_eq!(fs::read(out.join("doc/CITATION.cff")).unwrap(), b"nested\n");
}

#[test]
fn attachments_must_stay_in_their_folders() {
    for bad in [
        "../x",
        "src/x",
        "results/summary.json",
        "ro-crate-metadata.json",
        "doc//x",
        "doc/a\\b",
        "doc/",
        "/doc/x",
        "doc/a\nb",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let mut f = facts();
        f.attachments.push((bad.into(), b"x".to_vec()));
        let (problems, _) = refused(build(&fixture(), &tmp.path().join("out"), &f, &release()));
        assert_eq!(codes(&problems), ["E-PUB-010"], "{bad:?}");
        assert_eq!(problems.len(), 1, "{bad:?}");
        assert_eq!(
            problems[0].instruction,
            "Attach under a new relative path in doc/, records/ or results/."
        );
    }
    let tmp = tempfile::tempdir().unwrap();
    let mut f = facts();
    f.attachments.push(("doc/a.txt".into(), b"x".to_vec()));
    f.attachments.push(("doc/a.txt".into(), b"y".to_vec()));
    let (problems, _) = refused(build(&fixture(), &tmp.path().join("out"), &f, &release()));
    assert_eq!(codes(&problems), ["E-PUB-010"]);
    let tmp = tempfile::tempdir().unwrap();
    let mut f = facts();
    f.attachments.push(("doc/ok/a.txt".into(), b"x".to_vec()));
    let out = tmp.path().join("out");
    build(&fixture(), &out, &f, &release()).unwrap();
    assert_eq!(fs::read(out.join("doc/ok/a.txt")).unwrap(), b"x");
}

#[test]
fn attachments_never_nest_with_study_files_or_each_other() {
    let tmp = tempfile::tempdir().unwrap();
    let study = study_copy(tmp.path());
    fs::create_dir(study.join("results/sub")).unwrap();
    fs::write(study.join("results/sub/a.txt"), "a\n").unwrap();
    for (added, at) in [
        (vec!["results/summary.json/x"], "results/summary.json/x"),
        (vec!["results/sub"], "results/sub"),
        (vec!["records/a", "records/a/b"], "records/a"),
    ] {
        let mut f = facts();
        for a in added {
            f.attachments.push((a.into(), b"x".to_vec()));
        }
        let out = tmp.path().join("out");
        let (problems, _) = refused(build(&study, &out, &f, &release()));
        assert_eq!(problems.len(), 1, "{at}");
        assert_eq!(
            (problems[0].code, problems[0].path.as_str()),
            ("E-PUB-010", at)
        );
        assert!(!out.exists(), "{at}");
    }
}

#[test]
fn artifacts_are_hashed_and_listed() {
    let tmp = tempfile::tempdir().unwrap();
    let art = tmp.path().join("image.tar");
    fs::write(&art, "image\n").unwrap();
    let mut f = facts();
    f.artifacts = vec![art.clone()];
    let out = tmp.path().join("out");
    let built = build(&fixture(), &out, &f, &release()).unwrap();
    let sha = sha256_bytes(b"image\n");
    assert_eq!(
        fs::read_to_string(out.join("ARTIFACTS.sha256")).unwrap(),
        format!("{sha}  image.tar\n")
    );
    assert_eq!(built.artifacts.len(), 1);
    assert_eq!(built.artifacts[0].0.name, "image.tar");
    assert_eq!(built.artifacts[0].0.sha256, sha);
    assert_eq!(built.artifacts[0].0.size, 6);
    assert_eq!(built.artifacts[0].1, art);
    assert!(!out.join("image.tar").exists());
    let doc = json(&out.join("ro-crate-metadata.json"));
    let root = doc["@graph"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["@id"] == "./")
        .unwrap();
    let want = format!("#sha256-{sha}");
    assert!(
        root["mentions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["@id"] == want.as_str())
    );

    for name in [
        zip_name("fixture-numeric", "1.0.0"),
        "bad name.tar".to_string(),
    ] {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join(&name);
        fs::write(&p, "z").unwrap();
        let mut f = facts();
        f.artifacts = vec![p];
        let (problems, _) = refused(build(&fixture(), &t.path().join("out"), &f, &release()));
        assert_eq!(codes(&problems), ["E-PUB-010"], "{name}");
        assert_eq!(problems[0].path, name);
        assert_eq!(
            problems[0].instruction,
            "Name artifacts with letters, digits, dots, hyphens and underscores, each name once."
        );
    }
    let t = tempfile::tempdir().unwrap();
    let (a, b) = (t.path().join("x"), t.path().join("sub"));
    fs::create_dir(&b).unwrap();
    let b = b.join("x");
    fs::write(&a, "1").unwrap();
    fs::write(&b, "2").unwrap();
    let mut f = facts();
    f.artifacts = vec![a, b];
    let (problems, _) = refused(build(&fixture(), &t.path().join("out"), &f, &release()));
    assert_eq!(codes(&problems), ["E-PUB-010"]);
}

#[test]
fn unreadable_artifacts_are_problems() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("dir.tar");
    fs::create_dir(&dir).unwrap();
    let mut f = facts();
    f.artifacts = vec![t.path().join("absent.tar"), dir];
    f.label = "bogus".into();
    let (problems, _) = refused(build(&fixture(), &t.path().join("out"), &f, &release()));
    let got: Vec<(&str, &str, &str)> = problems
        .iter()
        .map(|p| (p.code, p.path.as_str(), p.message.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("E-PUB-014", "label", "is not a claim label"),
            ("E-PUB-010", "absent.tar", "cannot be read"),
            ("E-PUB-010", "dir.tar", "cannot be read"),
        ]
    );
    assert_eq!(
        problems[1].instruction,
        "Give the path of a regular file that can be read."
    );
}

#[test]
fn a_citation_that_is_not_utf8_is_a_problem() {
    let tmp = tempfile::tempdir().unwrap();
    let study = study_copy(tmp.path());
    fs::write(study.join("citation.toml"), b"title = \"\xFF\"\n").unwrap();
    let (problems, _) = refused(build(&study, &tmp.path().join("out"), &facts(), &release()));
    assert_eq!(problems.len(), 1);
    assert_eq!(
        (problems[0].code, problems[0].path.as_str()),
        ("E-PUB-004", "citation.toml")
    );
    assert_eq!(problems[0].message, "is not UTF-8 text");
}

#[test]
fn a_study_id_must_name_a_zip() {
    for bad in ["", "a b", "a/b", "\u{e9}", "a\nb"] {
        let tmp = tempfile::tempdir().unwrap();
        let mut f = facts();
        f.study_id = bad.into();
        let out = tmp.path().join("out");
        let (problems, _) = refused(build(&fixture(), &out, &f, &release()));
        assert_eq!(problems.len(), 1, "{bad:?}");
        let p = &problems[0];
        assert_eq!(
            (p.code, p.path.as_str(), p.message.as_str()),
            ("E-PUB-016", "study_id", "cannot name a bundle zip"),
            "{bad:?}"
        );
        assert_eq!(
            p.instruction,
            "Use letters, digits, dots, hyphens and underscores in the study id."
        );
        assert!(!out.exists());
    }
}

#[test]
fn zip_name_joins_id_and_version() {
    assert_eq!(
        zip_name("fixture-numeric", "1.0.0"),
        "fixture-numeric-1.0.0.zip"
    );
}

#[test]
fn an_existing_out_is_never_touched() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    fs::create_dir(&out).unwrap();
    fs::write(out.join("keep.txt"), "keep\n").unwrap();
    let r = build(&fixture(), &out, &facts(), &release());
    assert!(matches!(r, Err(Error::Io(_))), "{r:?}");
    assert_eq!(fs::read_to_string(out.join("keep.txt")).unwrap(), "keep\n");
    assert_eq!(fs::read_dir(&out).unwrap().count(), 1);
}

#[test]
fn spdx_headers_are_read_from_the_first_64_kib() {
    let tmp = tempfile::tempdir().unwrap();
    let study = study_copy(tmp.path());
    let long = format!(
        "# {}\n# SPDX-License-Identifier: Apache-2.0\n",
        "x".repeat(2000)
    );
    fs::write(study.join("src/long"), long).unwrap();
    // The 64 KiB cut falls inside `é`. The byte after it is not UTF-8, so a longer
    // read would skip the file as binary and give no warning.
    let mut cut = vec![b'a'; 65_535];
    cut.extend_from_slice("\u{e9}\n".as_bytes());
    cut.push(0xFF);
    fs::write(study.join("src/cut"), cut).unwrap();
    let built = build(&study, &tmp.path().join("out"), &facts(), &release()).unwrap();
    let got: Vec<(&str, &str)> = built
        .warnings
        .iter()
        .map(|w| (w.code, w.path.as_str()))
        .collect();
    assert_eq!(got, [("W-PUB-002", "src/cut")]);
}

#[test]
fn licence_warnings_ride_along() {
    let tmp = tempfile::tempdir().unwrap();
    let mut f = facts();
    f.licensing.inputs[0].path = "data/texts/cited-only.txt".into();
    f.licensing.inputs[0].licence.kind = Kind::RequestForm;
    f.runs[0].object = vec![];
    let built = build(&fixture(), &tmp.path().join("out"), &f, &release()).unwrap();
    assert_eq!(built.warnings.len(), 1);
    assert_eq!(built.warnings[0].code, "W-PUB-001");
}
