mod common;
use ignorarium_guest::tree::{TreeRow, hash_rows, row, tree_hash, tree_rows};
use sha2::Digest;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::process::Command;

/// The hash of the known tree: a file, a FIFO, a link and a file in a folder.
const KNOWN: &str = "d8ea3440790d5fd380cee5e278ab7172ad909ca7218742feb500d41b9a36dbcf";

fn digest(b: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(b))
}

/// SHA-256 of `rows` as compact JSON.
fn hash_of(rows: serde_json::Value) -> String {
    digest(&serde_json::to_vec(&rows).unwrap())
}

fn known_tree() -> tempfile::TempDir {
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
    d
}

fn tree_row(path: &str, kind: &'static str, digest: String) -> TreeRow {
    TreeRow {
        path: path.into(),
        kind,
        digest,
    }
}

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
    assert_eq!(tree_hash(known_tree().path()).unwrap(), KNOWN);
}

#[test]
fn tree_hash_is_hash_rows_of_tree_rows() {
    let d = known_tree();
    let rows = tree_rows(d.path()).unwrap();
    assert_eq!(
        rows,
        [
            tree_row("a.txt", "file", digest(b"hi\n")),
            tree_row("f", "other", String::new()),
            tree_row("l", "link", digest(b"a.txt")),
            tree_row("sub/b", "file", digest(b"yo\n")),
        ]
    );
    assert_eq!(hash_rows(&rows).unwrap(), KNOWN);
}

#[test]
fn hash_rows_ignores_row_order() {
    let a = row(b"a", "file", "01".into());
    let b = row(b"b\xff", "link", "02".into());
    let want = hash_of(serde_json::json!([
        ["!bytes:62ff", "link", "02"],
        ["a", "file", "01"]
    ]));
    assert_eq!(hash_rows(&[a.clone(), b.clone()]).unwrap(), want);
    assert_eq!(hash_rows(&[b, a]).unwrap(), want);
    assert_eq!(
        hash_rows(&[]).unwrap(),
        "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945"
    );
}

/// A link's row holds the SHA-256 of its target bytes; the target itself is never read.
#[test]
fn different_link_targets_give_different_hashes() {
    let tree = |target: &str| {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "hi\n").unwrap();
        symlink(target, d.path().join("l")).unwrap();
        d
    };
    let hash = |target: &str| tree_hash(tree(target).path()).unwrap();
    assert_ne!(hash("/etc/passwd"), hash("a.txt"));
    assert_eq!(
        tree_rows(tree("/etc/passwd").path()).unwrap()[1],
        tree_row("l", "link", digest(b"/etc/passwd"))
    );
}

#[test]
fn non_utf8_names_hash_as_bytes() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join(OsStr::from_bytes(b"x\xff")), "hi\n").unwrap();
    let rows = serde_json::json!([["!bytes:78ff", "file", digest(b"hi\n")]]);
    assert_eq!(tree_hash(d.path()).unwrap(), hash_of(rows));
}

#[test]
fn a_nested_non_utf8_name_hashes_part_by_part() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join(OsStr::from_bytes(b"d\xfe"));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join(OsStr::from_bytes(b"x\xff")), "hi\n").unwrap();
    let rows = serde_json::json!([["!bytes:64fe/!bytes:78ff", "file", digest(b"hi\n")]]);
    assert_eq!(tree_hash(d.path()).unwrap(), hash_of(rows));
}

#[test]
fn rows_with_the_same_path_text_sort_by_digest_as_in_level_0() {
    // Both names encode to `!bytes:ff`; the walk meets the UTF-8 one first.
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("!bytes:ff"), "a").unwrap();
    std::fs::write(d.path().join(OsStr::from_bytes(b"\xff")), "b").unwrap();
    let (a, b) = (digest(b"a"), digest(b"b"));
    assert!(b < a);
    assert_eq!(
        tree_rows(d.path()).unwrap(),
        [
            tree_row("!bytes:ff", "file", b.clone()),
            tree_row("!bytes:ff", "file", a.clone())
        ]
    );
    let rows = serde_json::json!([["!bytes:ff", "file", b], ["!bytes:ff", "file", a]]);
    assert_eq!(tree_hash(d.path()).unwrap(), hash_of(rows));
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
