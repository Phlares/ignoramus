mod common;

use ed25519_dalek::VerifyingKey;
use ignorarium::config;
use ignorarium::derived::{By, STAMP, STAMP_CHECKED, Stamp, StampLines};
use ignorarium::keys;
use ignorarium::records;
use ignorarium::stamp::{self, Line, StampError};
use ignorarium::tsp;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn key_of(lab: &common::Lab) -> VerifyingKey {
    keys::load(&lab.home).unwrap().unwrap().verifying_key()
}

fn seals_of(lab: &common::Lab, id: &str) -> Vec<records::Seal> {
    let log = lab.home.study_dir(id).join("records.jsonl");
    records::seals(&log, &key_of(lab)).unwrap()
}

fn request(lab: &common::Lab, id: &str) -> assert_cmd::assert::Assert {
    common::bin(lab).args(["stamp", id, "--request"]).assert()
}

fn run_study(lab: &common::Lab, id: &str) {
    common::bin(lab)
        .current_dir(common::work(lab, id))
        .args(["run", id])
        .assert()
        .code(0);
}

#[test]
fn the_register_line_has_the_frozen_form() {
    let lab = common::lab();
    common::plain(&lab, "s1");
    let text = stamp::lines(&lab.home, "s1", &key_of(&lab)).unwrap();
    let s = seals_of(&lab, "s1");
    let mut it = text.lines();
    assert_eq!(
        it.next().unwrap(),
        format!("register record hash of s1: {}", s[0].hash)
    );
    assert_eq!(
        it.next().unwrap(),
        format!("register record signature of s1: {}", s[0].sig)
    );
    assert_eq!(
        it.next().unwrap(),
        format!(
            "box public key (ed25519): {}",
            hex::encode(key_of(&lab).to_bytes())
        )
    );
    assert!(text.ends_with('\n') && !text.ends_with("\n\n"));
}

#[test]
fn the_prefixes_are_frozen() {
    assert_eq!(Line::Register.prefix("x"), "register record hash of x: ");
    assert_eq!(
        Line::RegisterSignature.prefix("x"),
        "register record signature of x: "
    );
    assert_eq!(Line::Key.prefix("x"), "box public key (ed25519): ");
    assert_eq!(Line::Head.prefix("x"), "record head of x: ");
    assert_eq!(
        Line::HeadSignature.prefix("x"),
        "record head signature of x: "
    );
}

#[test]
fn a_request_is_the_same_bytes_twice() {
    let lab = common::lab();
    common::plain(&lab, "s2");
    let key = key_of(&lab);
    let a = stamp::request(&lab.home, "s2", &key, None).unwrap();
    let b = stamp::request(&lab.home, "s2", &key, None).unwrap();
    assert_eq!(a, b);
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    let name = a.file_name().unwrap().to_str().unwrap();
    let want = ignorarium::canon::sha256_hex(&std::fs::read(&a).unwrap());
    assert_eq!(name, format!("{want}.txt"));
    assert_eq!(a.parent().unwrap(), lab.home.study_dir("s2").join("stamps"));
}

#[test]
fn the_head_is_the_last_record() {
    let lab = common::lab();
    common::plain(&lab, "s3");
    run_study(&lab, "s3");
    let s = seals_of(&lab, "s3");
    let last = s.last().unwrap();
    assert!(last.seq > 0);
    let text = stamp::lines(&lab.home, "s3", &key_of(&lab)).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[3],
        format!("record head of s3: seq {}, hash {}", last.seq, last.hash)
    );
    assert_eq!(
        lines[4],
        format!("record head signature of s3: {}", last.sig)
    );
}

#[test]
fn with_appends_the_authors_lines() {
    let lab = common::lab();
    common::plain(&lab, "s4");
    let key = key_of(&lab);
    let extra = lab.root.join("extra.txt");
    std::fs::write(&extra, "statement hash: abc\n").unwrap();
    let base = stamp::lines(&lab.home, "s4", &key).unwrap();
    let path = stamp::request(&lab.home, "s4", &key, Some(&extra)).unwrap();
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        format!("{base}statement hash: abc\n")
    );
}

#[test]
fn a_with_file_that_holds_another_box_line_is_refused_at_request() {
    let lab = common::lab();
    common::plain(&lab, "s6");
    let older = requested(&lab, "s6");
    // A later record moves the head: the older file's head line is not the box's now.
    stop(&lab, "s6");
    let study = lab.home.study_dir("s6");
    let before = common::files_under(&study);
    let a = common::bin(&lab)
        .args(["stamp", "s6", "--request", "--with"])
        .arg(&older)
        .assert()
        .code(1);
    assert!(err(&a).contains("E-STP-003"), "{}", err(&a));
    assert_eq!(common::files_under(&study), before);
}

#[test]
fn a_request_appends_no_record() {
    let lab = common::lab();
    common::plain(&lab, "s5");
    let log = lab.home.study_dir("s5").join("records.jsonl");
    let before = std::fs::read(&log).unwrap();
    request(&lab, "s5").code(0);
    assert_eq!(before, std::fs::read(&log).unwrap());
}

#[test]
fn stamps_is_a_private_folder() {
    use std::os::unix::fs::PermissionsExt;
    let lab = common::lab();
    common::plain(&lab, "s6");
    request(&lab, "s6").code(0);
    let m = std::fs::symlink_metadata(lab.home.study_dir("s6").join("stamps")).unwrap();
    assert!(m.is_dir());
    assert_eq!(m.permissions().mode() & 0o777, 0o700);
}

#[test]
fn a_link_for_stamps_is_refused() {
    let lab = common::lab();
    common::plain(&lab, "s7");
    let elsewhere = lab.root.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, lab.home.study_dir("s7").join("stamps")).unwrap();
    request(&lab, "s7").code(2);
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn a_study_that_is_not_registered_is_refused() {
    let lab = common::lab();
    common::plain(&lab, "s8");
    request(&lab, "nope").code(1);
}

#[test]
fn the_plan_log_head_is_the_last_line_of_heads() {
    let lab = common::lab();
    common::plain(&lab, "s9");
    common::plain(&lab, "s9b");
    let key = key_of(&lab);
    let heads = lab.home.root.join("planlog").join("heads.jsonl");
    let text = std::fs::read_to_string(&heads).unwrap();
    assert_eq!(text.lines().count(), 2);
    let last = text.lines().last().unwrap().to_string();
    let got = stamp::lines(&lab.home, "s9", &key).unwrap();
    assert_eq!(
        got.lines().last().unwrap(),
        format!("plan log head (planlog): {last}")
    );
    assert_eq!(got.lines().count(), 6);
    std::fs::write(&heads, "").unwrap();
    let got = stamp::lines(&lab.home, "s9", &key).unwrap();
    assert_eq!(got.lines().count(), 5);
    std::fs::rename(&heads, heads.with_extension("moved")).unwrap();
    let got = stamp::lines(&lab.home, "s9", &key).unwrap();
    assert_eq!(got.lines().count(), 5);
}

#[test]
fn the_commands_name_each_authority() {
    let lab = common::lab();
    common::plain(&lab, "c1");
    std::fs::write(
        lab.home.root.join("config.toml"),
        "[stamp]\nauthorities = [\"https://one.example/tsr\", \"https://two.example/tsr\"]\n",
    )
    .unwrap();
    let out = common::text(&request(&lab, "c1").code(0));
    let lines: Vec<&str> = out.lines().collect();
    let file = lines[0];
    assert!(PathBuf::from(file).is_file());
    let curls: Vec<&&str> = lines.iter().filter(|l| l.starts_with("curl")).collect();
    assert_eq!(curls.len(), 2);
    assert!(curls[0].contains("https://one.example/tsr"));
    assert!(curls[0].contains(&format!("--data-binary @\"{file}.tsq\"")));
    assert!(curls[0].ends_with(&format!("-o \"{file}.1.tsr\"")));
    assert!(curls[1].contains("https://two.example/tsr"));
    assert!(curls[1].ends_with(&format!("-o \"{file}.2.tsr\"")));
    assert!(out.contains(&format!(
        "openssl ts -query -data \"{file}\" -sha256 -cert -out \"{file}.tsq\""
    )));
    assert!(out.contains(&format!("ignorarium stamp c1 --reply \"{file}.2.tsr\"")));
    assert!(!out.contains("no authority"));
}

#[test]
fn with_no_authority_the_placeholder_and_a_notice_print() {
    let lab = common::lab();
    common::plain(&lab, "c2");
    let out = common::text(&request(&lab, "c2").code(0));
    assert!(out.contains("no authority is set in config.toml"));
    assert!(out.contains("<authority url>"));
    assert_eq!(out.lines().filter(|l| l.starts_with("curl")).count(), 1);
}

#[test]
fn the_config_reads_as_a_machine_file_does() {
    let lab = common::lab();
    assert_eq!(
        config::load(&lab.home.root).unwrap(),
        config::Config::default()
    );
    std::fs::write(
        lab.home.root.join("config.toml"),
        "[stamp]\nauthority = []\n",
    )
    .unwrap();
    assert!(config::load(&lab.home.root).is_err());
}

#[test]
fn a_config_that_cannot_be_read_is_an_error_not_the_defaults() {
    let lab = common::lab();
    std::fs::create_dir(lab.home.root.join("config.toml")).unwrap();
    assert!(config::load(&lab.home.root).is_err());
}

#[test]
fn with_needs_request() {
    let lab = common::lab();
    common::plain(&lab, "u1");
    common::bin(&lab).args(["stamp", "u1"]).assert().code(2);
    common::bin(&lab)
        .args(["stamp", "u1", "--with", "x"])
        .assert()
        .code(2);
    for args in [
        &["--request", "--reply", "r"][..],
        &["--file", "f"],
        &["--request", "--file", "f"],
        &["--reply", "r", "--with", "x"],
    ] {
        common::bin(&lab)
            .args(["stamp", "u1"])
            .args(args)
            .assert()
            .code(2);
    }
}

#[test]
fn a_request_works_on_a_done_study() {
    let lab = common::lab();
    common::plain(&lab, "d1");
    stop(&lab, "d1");
    assert!(
        common::records_of(&lab, "d1")
            .iter()
            .any(|r| r.kind == "done")
    );
    let file = requested(&lab, "d1");
    assert!(file.is_file());
}

#[test]
fn a_request_that_fails_writes_nothing() {
    let lab = common::lab();
    common::plain(&lab, "f1");
    let stamps = lab.home.study_dir("f1").join("stamps");
    common::bin(&lab)
        .args(["stamp", "f1", "--request", "--with"])
        .arg(lab.root.join("missing.txt"))
        .assert()
        .failure();
    assert!(!stamps.exists());
    let log = lab.home.study_dir("f1").join("records.jsonl");
    let text = std::fs::read_to_string(&log).unwrap();
    std::fs::write(&log, text.replacen("\"register\"", "\"registeR\"", 1)).unwrap();
    request(&lab, "f1").failure();
    assert!(!stamps.exists());
}

#[test]
fn an_altered_plan_log_head_fails_the_request() {
    let lab = common::lab();
    common::plain(&lab, "p1");
    let heads = lab.home.root.join("planlog").join("heads.jsonl");
    let text = std::fs::read_to_string(&heads).unwrap();
    let at = text.find("\"root\":\"").unwrap() + "\"root\":\"".len();
    let flip = if &text[at..=at] == "0" { "1" } else { "0" };
    let changed = format!("{}{flip}{}", &text[..at], &text[at + 1..]);
    let torn = text[..text.len() / 2].to_string();
    let stamps = lab.home.study_dir("p1").join("stamps");
    for bad in [changed, torn] {
        std::fs::write(&heads, &bad).unwrap();
        let a = request(&lab, "p1").failure();
        assert!(err(&a).contains("planlog"), "{}", err(&a));
        assert!(!stamps.exists());
    }
}

const UNIX: u64 = 1_794_350_509;

fn sha(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn err(a: &assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&a.get_output().stderr).into_owned()
}

fn stop(lab: &common::Lab, id: &str) {
    common::bin(lab)
        .args(["stop", id, "--reason", "enough"])
        .assert()
        .code(0);
}

/// `stamp --request`, and the file it wrote.
fn requested(lab: &common::Lab, id: &str) -> PathBuf {
    let out = common::text(&request(lab, id).code(0));
    PathBuf::from(out.lines().next().unwrap())
}

/// A granted reply over `bytes` at `unix`, written to `name` in the lab.
fn reply_over(lab: &common::Lab, name: &str, bytes: &[u8], unix: u64) -> PathBuf {
    let path = lab.root.join(name);
    std::fs::write(&path, tsp::fixture(&sha(bytes), unix, Some("CN=T"))).unwrap();
    path
}

fn reply(lab: &common::Lab, id: &str, r: &Path, file: Option<&Path>) -> assert_cmd::assert::Assert {
    let mut c = common::bin(lab);
    c.args(["stamp", id, "--reply"]).arg(r);
    if let Some(f) = file {
        c.arg("--file").arg(f);
    }
    c.assert()
}

fn stamps_of(lab: &common::Lab, id: &str) -> Vec<Stamp> {
    common::records_of(lab, id)
        .iter()
        .filter(|r| r.kind == STAMP)
        .map(|r| Stamp::of(r).unwrap())
        .collect()
}

/// A file of `text` in the lab, and a reply over it.
fn file_and_reply(lab: &common::Lab, name: &str, text: &str) -> (PathBuf, PathBuf) {
    let f = lab.root.join(format!("{name}.txt"));
    std::fs::write(&f, text).unwrap();
    let r = reply_over(lab, &format!("{name}.tsr"), text.as_bytes(), UNIX);
    (f, r)
}

fn register_line(lab: &common::Lab, id: &str) -> String {
    format!(
        "register record hash of {id}: {}\n",
        seals_of(lab, id)[0].hash
    )
}

#[test]
fn a_reply_is_recorded() {
    let lab = common::lab();
    common::plain(&lab, "r1");
    let file = requested(&lab, "r1");
    let bytes = std::fs::read(&file).unwrap();
    let r = reply_over(&lab, "r1.tsr", &bytes, UNIX);
    let a = reply(&lab, "r1", &r, None).code(0);
    let all = common::records_of(&lab, "r1");
    let last = all.last().unwrap();
    assert_eq!(last.kind, STAMP);
    assert_eq!(common::text(&a), format!("stamp record: {}\n", last.seq));
    let s = Stamp::of(last).unwrap();
    let reply_bytes = std::fs::read(&r).unwrap();
    assert_eq!(s.file_sha256, hex::encode(sha(&bytes)));
    assert_eq!(s.reply_sha256, hex::encode(sha(&reply_bytes)));
    assert_eq!(s.stamped_at, "2026-11-10T22:41:49.000Z");
    assert_eq!(s.authority.as_deref(), Some("CN=T"));
    assert_eq!(s.serial, "01");
    assert_eq!(s.policy, "1.2.3.4.1");
    assert_eq!(s.checked, STAMP_CHECKED);
    let seals = seals_of(&lab, "r1");
    let head = &seals[seals.len() - 2];
    assert_eq!(
        s.lines,
        StampLines {
            register: true,
            register_signature: true,
            key: true,
            head: Some(By {
                seq: head.seq,
                hash: head.hash.clone()
            }),
            head_signature: true,
        }
    );
    let dir = lab.home.study_dir("r1").join("stamps");
    assert_eq!(
        std::fs::read(dir.join(format!("{}.txt", s.file_sha256))).unwrap(),
        bytes
    );
    assert_eq!(
        std::fs::read(dir.join(format!("{}.tsr", s.reply_sha256))).unwrap(),
        reply_bytes
    );
}

#[test]
fn a_file_with_only_the_register_line_records() {
    let lab = common::lab();
    common::plain(&lab, "r2");
    let (f, r) = file_and_reply(&lab, "only", &register_line(&lab, "r2"));
    reply(&lab, "r2", &r, Some(&f)).code(0);
    let s = stamps_of(&lab, "r2");
    assert_eq!(s.len(), 1);
    assert_eq!(
        s[0].lines,
        StampLines {
            register: true,
            register_signature: false,
            key: false,
            head: None,
            head_signature: false,
        }
    );
    let copy = lab
        .home
        .study_dir("r2")
        .join("stamps")
        .join(format!("{}.txt", s[0].file_sha256));
    assert_eq!(std::fs::read(copy).unwrap(), std::fs::read(&f).unwrap());
}

#[test]
fn each_refusal_writes_nothing() {
    let lab = common::lab();
    common::plain(&lab, "r3");
    let file = requested(&lab, "r3");
    let reg = register_line(&lab, "r3");
    let zeros = "0".repeat(64);
    let not_granted = lab.root.join("rejected.tsr");
    std::fs::write(&not_granted, [0x30, 0x05, 0x30, 0x03, 0x02, 0x01, 0x02]).unwrap();
    let other = reply_over(&lab, "other.tsr", b"another file", UNIX);
    // The SHA-256 OID with its last arc made 2: SHA-384.
    let sha384 = lab.root.join("sha384.tsr");
    let mut der = tsp::fixture(&sha(&std::fs::read(&file).unwrap()), UNIX, None);
    let oid = [
        0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01,
    ];
    let at = der.windows(oid.len()).position(|w| w == oid).unwrap();
    der[at + oid.len() - 1] = 0x02;
    std::fs::write(&sha384, der).unwrap();
    let cases: Vec<(&str, &str, PathBuf, Option<PathBuf>)> = vec![
        ("E-STP-001", "not granted", not_granted, None),
        ("E-STP-001", "digest algorithm", sha384, None),
        ("E-STP-002", "no file", other.clone(), None),
        ("E-STP-002", "has SHA-256", other, Some(file.clone())),
        {
            let (f, r) = file_and_reply(&lab, "noreg", "hello\n");
            ("E-STP-003", "no line", r, Some(f))
        },
        {
            let (f, r) = file_and_reply(
                &lab,
                "otherreg",
                &format!("register record hash of r3: {zeros}\n"),
            );
            ("E-STP-003", "not the box's line", r, Some(f))
        },
        {
            let (f, r) = file_and_reply(
                &lab,
                "badhead",
                &format!("{reg}record head of r3: seq 0, hash {zeros}\n"),
            );
            ("E-STP-004", "names no record", r, Some(f))
        },
        {
            let (f, r) = file_and_reply(
                &lab,
                "badkey",
                &format!("{reg}box public key (ed25519): {zeros}\n"),
            );
            ("E-STP-003", "not the box's line", r, Some(f))
        },
    ];
    let study = lab.home.study_dir("r3");
    let before = common::files_under(&study);
    for (code, why, r, f) in cases {
        let a = reply(&lab, "r3", &r, f.as_deref()).code(1);
        assert!(err(&a).contains(code), "{code}: {}", err(&a));
        assert!(err(&a).contains(why), "{code}: {}", err(&a));
        assert_eq!(common::files_under(&study), before, "{code}");
    }
}

#[test]
fn the_wrong_file_as_a_reply_is_e_stp_001() {
    let lab = common::lab();
    common::plain(&lab, "r4");
    let file = requested(&lab, "r4");
    let html = lab.root.join("page.tsr");
    std::fs::write(&html, "<!doctype html><html><body>502</body></html>").unwrap();
    let empty = lab.root.join("empty.tsr");
    std::fs::write(&empty, "").unwrap();
    let study = lab.home.study_dir("r4");
    let before = common::files_under(&study);
    for r in [file, html, empty] {
        let a = reply(&lab, "r4", &r, None).code(1);
        assert!(err(&a).contains("E-STP-001"), "{}", err(&a));
        assert!(err(&a).contains("not a time-stamp reply"), "{}", err(&a));
    }
    assert_eq!(common::files_under(&study), before);
}

#[test]
fn a_crlf_file_is_refused_and_names_the_line() {
    let lab = common::lab();
    common::plain(&lab, "r5");
    let text = std::fs::read_to_string(requested(&lab, "r5")).unwrap();
    let (f, r) = file_and_reply(&lab, "crlf", &text.replace('\n', "\r\n"));
    let a = reply(&lab, "r5", &r, Some(&f)).code(1);
    let want = register_line(&lab, "r5");
    assert!(err(&a).contains("E-STP-003"), "{}", err(&a));
    assert!(err(&a).contains(want.trim_end()), "{}", err(&a));
    assert!(stamps_of(&lab, "r5").is_empty());
}

#[test]
fn the_same_reply_twice_is_one_record() {
    let lab = common::lab();
    common::plain(&lab, "r6");
    let bytes = std::fs::read(requested(&lab, "r6")).unwrap();
    let r = reply_over(&lab, "r6.tsr", &bytes, UNIX);
    reply(&lab, "r6", &r, None).code(0);
    let all = common::records_of(&lab, "r6");
    let a = reply(&lab, "r6", &r, None).code(0);
    assert_eq!(common::text(&a), "already recorded\n");
    assert_eq!(common::records_of(&lab, "r6"), all);
}

#[test]
fn two_replies_are_two_records_in_order() {
    let lab = common::lab();
    common::plain(&lab, "r7");
    let bytes = std::fs::read(requested(&lab, "r7")).unwrap();
    let one = reply_over(&lab, "one.tsr", &bytes, UNIX);
    let two = reply_over(&lab, "two.tsr", &bytes, UNIX + 60);
    reply(&lab, "r7", &one, None).code(0);
    reply(&lab, "r7", &two, None).code(0);
    let s = stamps_of(&lab, "r7");
    assert_eq!(s.len(), 2);
    assert_eq!(
        s[0].reply_sha256,
        hex::encode(sha(&std::fs::read(&one).unwrap()))
    );
    assert_eq!(
        s[1].reply_sha256,
        hex::encode(sha(&std::fs::read(&two).unwrap()))
    );
    assert_eq!(s[0].file_sha256, s[1].file_sha256);
}

#[test]
fn records_between_request_and_reply_do_not_matter() {
    let lab = common::lab();
    common::plain(&lab, "r8");
    let old = seals_of(&lab, "r8").last().unwrap().clone();
    let bytes = std::fs::read(requested(&lab, "r8")).unwrap();
    run_study(&lab, "r8");
    assert!(seals_of(&lab, "r8").last().unwrap().seq > old.seq);
    let r = reply_over(&lab, "r8.tsr", &bytes, UNIX);
    reply(&lab, "r8", &r, None).code(0);
    let s = stamps_of(&lab, "r8");
    assert_eq!(
        s[0].lines.head,
        Some(By {
            seq: old.seq,
            hash: old.hash
        })
    );
    assert!(s[0].lines.head_signature);
}

#[test]
fn a_stamp_after_done_is_allowed() {
    let lab = common::lab();
    common::plain(&lab, "r9");
    stop(&lab, "r9");
    let bytes = std::fs::read(requested(&lab, "r9")).unwrap();
    let r = reply_over(&lab, "r9.tsr", &bytes, UNIX);
    reply(&lab, "r9", &r, None).code(0);
    let kinds: Vec<String> = common::records_of(&lab, "r9")
        .into_iter()
        .map(|r| r.kind)
        .collect();
    assert_eq!(&kinds[kinds.len() - 2..], ["done", STAMP]);
    common::bin(&lab)
        .args(["verify", "--study", "r9"])
        .assert()
        .code(0);
}

/// The code of a refusal of `found`, or the lines it found.
fn found_of(file: &str) -> Result<StampLines, &'static str> {
    let seals = [
        records::Seal {
            seq: 0,
            hash: "h0".into(),
            sig: "g0".into(),
        },
        records::Seal {
            seq: 1,
            hash: "h1".into(),
            sig: "g1".into(),
        },
    ];
    stamp::found(file.as_bytes(), "x", &seals, "k").map_err(|e| match e {
        StampError::Refused { code, .. } => code,
        e => panic!("{e}"),
    })
}

#[test]
fn found_checks_each_line_against_the_log() {
    let reg = "register record hash of x: h0\n";
    let all = format!(
        "{reg}register record signature of x: g0\nbox public key (ed25519): k\n\
         record head of x: seq 1, hash h1\nrecord head signature of x: g1\n"
    );
    let head = |seq: u64, hash: &str| {
        Some(By {
            seq,
            hash: hash.into(),
        })
    };
    let lines = |rs, k, h, hs| StampLines {
        register: true,
        register_signature: rs,
        key: k,
        head: h,
        head_signature: hs,
    };
    assert_eq!(found_of(&all), Ok(lines(true, true, head(1, "h1"), true)));
    assert_eq!(found_of(reg), Ok(lines(false, false, None, false)));
    assert_eq!(
        found_of(&format!("{reg}record head of x: seq 0, hash h0\n")),
        Ok(lines(false, false, head(0, "h0"), false))
    );
    // Lines of another study or another kind are not the box's.
    assert_eq!(
        found_of(&format!("{reg}register record hash of x2: zz\nnote: y\n")),
        Ok(lines(false, false, None, false))
    );
    assert_eq!(
        found_of(&format!("{reg}{reg}")),
        Ok(lines(false, false, None, false))
    );
    for (file, code) in [
        ("", "E-STP-003"),
        ("register record hash of x: h1\n", "E-STP-003"),
        ("register record hash of x: h0\r\n", "E-STP-003"),
        (
            &format!("{reg}register record hash of x: h1\n"),
            "E-STP-003",
        ),
        (
            &format!("{reg}register record signature of x: g1\n"),
            "E-STP-003",
        ),
        (&format!("{reg}box public key (ed25519): q\n"), "E-STP-003"),
        (
            &format!("{reg}record head of x: seq 1, hash h0\n"),
            "E-STP-004",
        ),
        (
            &format!("{reg}record head of x: seq 2, hash h1\n"),
            "E-STP-004",
        ),
        (
            &format!("{reg}record head of x: seq 01, hash h1\n"),
            "E-STP-004",
        ),
        (&format!("{reg}record head of x: 1 h1\n"), "E-STP-004"),
        (
            &format!("{reg}record head of x: seq 1, hash h1\nrecord head signature of x: g0\n"),
            "E-STP-003",
        ),
        (
            &format!("{reg}record head signature of x: g1\n"),
            "E-STP-003",
        ),
    ] {
        assert_eq!(found_of(file), Err(code), "{file:?}");
    }
}

#[test]
fn a_request_with_no_plan_log_folder_makes_none() {
    let lab = common::lab();
    common::plain(&lab, "m1");
    let folder = lab.home.root.join("planlog");
    std::fs::rename(&folder, lab.root.join("planlog-aside")).unwrap();
    // Through the library: every verb opens the home, which makes the folder again.
    let file = stamp::request(&lab.home, "m1", &key_of(&lab), None).unwrap();
    assert!(!folder.exists());
    let text = std::fs::read_to_string(file).unwrap();
    assert!(text.starts_with("register record hash of m1: "), "{text}");
    assert!(!text.contains("plan log head"), "{text}");
}

#[test]
fn a_file_that_cannot_be_read_is_an_io_fault_not_e_stp_002() {
    let lab = common::lab();
    common::plain(&lab, "g2");
    let r = reply_over(&lab, "g2.tsr", b"x", UNIX);
    let a = reply(&lab, "g2", &r, Some(&lab.root)).code(2);
    assert!(!err(&a).contains("E-STP-002"), "{}", err(&a));
    assert!(stamps_of(&lab, "g2").is_empty());
}

#[test]
fn a_digest_that_is_not_32_bytes_is_e_stp_001() {
    let lab = common::lab();
    common::plain(&lab, "g1");
    let study = lab.home.study_dir("g1");
    let before = common::files_under(&study);
    for n in [0, 31, 33, 4096] {
        let r = lab.root.join(format!("long{n}.tsr"));
        std::fs::write(&r, tsp::fixture(&vec![7; n], UNIX, None)).unwrap();
        let a = reply(&lab, "g1", &r, None).code(1);
        assert!(err(&a).contains("E-STP-001"), "{n}: {}", err(&a));
        assert!(err(&a).contains("digest length"), "{n}: {}", err(&a));
        assert_eq!(common::files_under(&study), before, "{n}");
    }
}

/// `verify --study` after a stamp made at `unix`: its stdout, and the stamp record.
fn verified_after(lab: &common::Lab, id: &str, unix: u64) -> (String, Stamp, u64) {
    let bytes = std::fs::read(requested(lab, id)).unwrap();
    let r = lab.root.join(format!("{id}.tsr"));
    std::fs::write(&r, tsp::fixture(&sha(&bytes), unix, Some("CN=T"))).unwrap();
    reply(lab, id, &r, None).code(0);
    let out = common::bin(lab)
        .args(["verify", "--study", id])
        .assert()
        .code(0);
    let seq = common::records_of(lab, id)
        .iter()
        .find(|r| r.kind == STAMP)
        .unwrap()
        .seq;
    (common::text(&out), stamps_of(lab, id).remove(0), seq)
}

#[test]
fn verify_prints_each_stamp() {
    let lab = common::lab();
    common::plain(&lab, "v1");
    stop(&lab, "v1");
    let (out, s, seq) = verified_after(&lab, "v1", 4_000_000_000);
    let head = s.lines.head.as_ref().unwrap();
    let line = format!(
        "stamp {seq}: {}, CN=T; lines: register yes, signature yes, key yes, head {} {}, head signature yes; file {}, reply {}
",
        s.stamped_at, head.seq, head.hash, s.file_sha256, s.reply_sha256
    );
    assert!(out.contains(&line), "{out}");
    assert!(!out.contains("flag:"), "{out}");
    assert!(out.contains("done: stop at "), "{out}");
    // The reader compares the stamped file's lines with these hashes.
    let register = format!("register record hash: {}\n", seals_of(&lab, "v1")[0].hash);
    assert!(out.contains(&register), "{out}");
}

/// An authority name that holds a line break, a terminal escape and a bidirectional control.
const FORGING: &str = "CN=a\nstamp 9: forged\u{1b}[2J\u{202e}";

#[test]
fn verify_escapes_the_authority() {
    let lab = common::lab();
    common::plain(&lab, "v3");
    let body = serde_json::json!({
        "file_sha256": "01".repeat(32), "reply_sha256": "a1".repeat(32),
        "authority": FORGING, "stamped_at": "2099-01-01T00:00:00.000Z", "serial": "1",
        "policy": "1.2.3",
        "lines": { "register": true, "register_signature": true, "key": true, "head": null,
                   "head_signature": false },
        "checked": ignorarium::derived::STAMP_CHECKED,
    });
    common::plant(&lab.home, "v3", &[("stamp", body)]);
    let out = common::text(
        &common::bin(&lab)
            .args(["verify", "--study", "v3"])
            .assert()
            .code(0),
    );
    let stamps: Vec<&str> = out.lines().filter(|l| l.starts_with("stamp ")).collect();
    assert_eq!(stamps.len(), 1, "{out}");
    assert!(!stamps[0].starts_with("stamp 9: forged"), "{out}");
    assert!(
        stamps[0].contains(r"CN=a\nstamp 9: forged\u{1b}[2J\u{202e};"),
        "{out}"
    );
    assert!(!out.contains(['\u{1b}', '\u{202e}']), "{out}");
    // The record keeps the authority as the reply stated it.
    assert_eq!(stamps_of(&lab, "v3")[0].authority.as_deref(), Some(FORGING));
}

/// A `stamp` body dated `at` whose lines name `head`.
fn stamp_body(at: &str, head: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "file_sha256": "01".repeat(32), "reply_sha256": "a1".repeat(32), "authority": "CN=T",
        "stamped_at": at, "serial": "1", "policy": "1.2.3",
        "lines": { "register": true, "register_signature": true, "key": true, "head": head,
                   "head_signature": !head.is_null() },
        "checked": STAMP_CHECKED,
    })
}

#[test]
fn verify_fails_on_a_stamp_head_the_log_does_not_hold() {
    let lab = common::lab();
    /// The head of a stamp, from the hash of the register record.
    type Head = fn(&str) -> serde_json::Value;
    let cases: [(&str, Head); 3] = [
        ("h1", |reg| serde_json::json!({ "seq": 999, "hash": reg })),
        (
            "h2",
            |_| serde_json::json!({ "seq": 0, "hash": "ab".repeat(32) }),
        ),
        // Record 1 is the stamp itself, whose own hash it cannot hold.
        ("h3", |reg| serde_json::json!({ "seq": 1, "hash": reg })),
    ];
    for (id, head) in cases {
        common::plain(&lab, id);
        let reg = seals_of(&lab, id)[0].hash.clone();
        let body = stamp_body("2099-01-01T00:00:00.000Z", head(&reg));
        common::plant(&lab.home, id, &[("stamp", body)]);
        let a = common::bin(&lab)
            .args(["verify", "--study", id])
            .assert()
            .code(1);
        assert!(
            err(&a).contains(&format!(
                "study {id}: record 1: the stamp record names no earlier record "
            )),
            "{id}: {}",
            err(&a)
        );
    }
}

#[test]
fn a_stamp_in_the_second_of_its_head_record_is_not_flagged() {
    let lab = common::lab();
    common::plain(&lab, "w1");
    let all = common::records_of(&lab, "w1");
    let reg = seals_of(&lab, "w1")[0].hash.clone();
    let head = By {
        seq: 0,
        hash: reg.clone(),
    };
    let mut log = all.clone();
    log[0].ts = "2026-11-10T12:00:00.500Z".into();
    let flag = |at: &str| {
        let body = stamp_body(at, serde_json::to_value(&head).unwrap());
        let s: Stamp = serde_json::from_value(body).unwrap();
        ignorarium::derived::stamp_flag(&log, &s)
    };
    assert_eq!(flag("2026-11-10T12:00:00.000Z"), None);
    assert_eq!(
        flag("2026-11-10T11:59:59.000Z").as_deref(),
        Some("dated before record 0, which it covers")
    );
}

#[test]
fn verify_flags_a_stamp_dated_before_its_record_and_exits_0() {
    let lab = common::lab();
    common::plain(&lab, "v2");
    let (out, s, seq) = verified_after(&lab, "v2", 1);
    let head = s.lines.head.as_ref().unwrap().seq;
    let flag = format!(
        "flag: stamp {seq} is dated before record {head}, which it covers
"
    );
    assert!(out.contains(&flag), "{out}");
}
