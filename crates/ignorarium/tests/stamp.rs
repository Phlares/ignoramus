mod common;

use ed25519_dalek::VerifyingKey;
use ignorarium::config;
use ignorarium::keys;
use ignorarium::records;
use ignorarium::stamp::{self, Line};
use std::path::PathBuf;

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
    let key = key_of(&lab);
    let heads = lab.home.root.join("planlog").join("heads.jsonl");
    let text = std::fs::read_to_string(&heads).unwrap();
    let last = text.lines().last().unwrap().to_string();
    std::fs::write(
        &heads,
        format!(
            "{{\"older\":1}}
{text}"
        ),
    )
    .unwrap();
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
    assert!(curls[0].ends_with(&format!("-o {file}.1.tsr")));
    assert!(curls[1].contains("https://two.example/tsr"));
    assert!(curls[1].ends_with(&format!("-o {file}.2.tsr")));
    assert!(out.contains(&format!(
        "openssl ts -query -data {file} -sha256 -cert -out {file}.tsq"
    )));
    assert!(out.contains(&format!("ignorarium stamp c1 --reply {file}.2.tsr")));
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
fn with_needs_request() {
    let lab = common::lab();
    common::plain(&lab, "u1");
    common::bin(&lab).args(["stamp", "u1"]).assert().code(2);
    common::bin(&lab)
        .args(["stamp", "u1", "--with", "x"])
        .assert()
        .code(2);
}
