mod common;
use ignorarium_guest::tree::tree_hash;
use sha2::Digest;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::process::Command;

#[test]
fn empty_dir_hash() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        tree_hash(d.path()).unwrap(),
        "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945"
    );
}

#[test]
fn known_tree_hash() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    std::fs::write(p.join("a.txt"), "hi\n").unwrap();
    std::fs::create_dir(p.join("sub")).unwrap();
    std::fs::write(p.join("sub/b"), "yo\n").unwrap();
    symlink("a.txt", p.join("l")).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(p.join("f"))
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        tree_hash(p).unwrap(),
        "60bd24f0904764b203f93cef9e02a36c8637682a27441cf9db6158faa29833d5"
    );
}

#[test]
fn links_are_not_followed() {
    let hash = |target: &str| {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "hi\n").unwrap();
        symlink(target, d.path().join("l")).unwrap();
        tree_hash(d.path()).unwrap()
    };
    assert_eq!(hash("/etc/passwd"), hash("a.txt"));
}

#[test]
fn non_utf8_names_hash_as_bytes() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join(OsStr::from_bytes(b"x\xff")), "hi\n").unwrap();
    let digest = hex::encode(sha2::Sha256::digest(b"hi\n"));
    let rows = serde_json::json!([["!bytes:78ff", "file", digest]]);
    let want = hex::encode(sha2::Sha256::digest(serde_json::to_vec(&rows).unwrap()));
    assert_eq!(tree_hash(d.path()).unwrap(), want);
}

#[test]
fn a_linked_top_dir_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("real")).unwrap();
    symlink("real", d.path().join("top")).unwrap();
    assert!(tree_hash(&d.path().join("top")).is_err());
}

#[test]
fn write_exec_makes_a_runnable_file() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("s");
    common::write_exec(&f, "#!/bin/sh\nexit 3\n");
    assert_eq!(Command::new(&f).status().unwrap().code(), Some(3));
}
