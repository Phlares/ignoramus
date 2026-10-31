use ignorarium::payload::{PayloadError, tree_hash};
use sha2::{Digest, Sha256};
use std::path::Path;

fn sha(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn put(dir: &Path, rel: &str, bytes: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

fn tree() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    put(d.path(), "run", b"#!/bin/sh\n");
    put(d.path(), "lib/measure.py", b"x = 1\n");
    d
}

#[test]
fn the_hash_covers_every_file() {
    let d = tree();
    let before = tree_hash(d.path()).unwrap();
    put(d.path(), "lib/measure.py", b"x = 2\n");
    let after = tree_hash(d.path()).unwrap();
    assert_ne!(before, after);
    put(d.path(), "lib/measure.py", b"x = 1\n");
    assert_eq!(tree_hash(d.path()).unwrap(), before);
    put(d.path(), "lib/extra", b"");
    assert_ne!(tree_hash(d.path()).unwrap(), before);
}

#[test]
fn the_hash_is_the_digest_of_the_sorted_manifest() {
    let d = tree();
    let manifest = format!(
        "[[\"lib/measure.py\",\"file\",\"{}\"],[\"run\",\"file\",\"{}\"]]",
        sha(b"x = 1\n"),
        sha(b"#!/bin/sh\n")
    );
    assert_eq!(tree_hash(d.path()).unwrap(), sha(manifest.as_bytes()));
}

#[test]
fn a_link_or_fifo_anywhere_is_refused() {
    let d = tree();
    std::os::unix::fs::symlink("/etc/hostname", d.path().join("lib/x")).unwrap();
    let Err(PayloadError::Unsafe(p)) = tree_hash(d.path()) else {
        panic!("a link must be refused");
    };
    assert_eq!(p, d.path().join("lib/x").display().to_string());
    std::fs::remove_file(d.path().join("lib/x")).unwrap();
    let fifo = d.path().join("f");
    let made = std::process::Command::new("mkfifo").arg(&fifo).status();
    assert!(made.unwrap().success());
    let Err(PayloadError::Unsafe(p)) = tree_hash(d.path()) else {
        panic!("a fifo must be refused");
    };
    assert_eq!(p, fifo.display().to_string());
}

#[test]
fn a_linked_folder_is_refused() {
    let d = tree();
    let ln = d.path().join("ln");
    std::os::unix::fs::symlink(d.path(), &ln).unwrap();
    let Err(PayloadError::Unsafe(p)) = tree_hash(&ln) else {
        panic!("a linked folder must be refused");
    };
    assert_eq!(p, ln.display().to_string());
}

#[test]
fn a_folder_without_run_is_refused() {
    let d = tempfile::tempdir().unwrap();
    put(d.path(), "lib/measure.py", b"x");
    assert!(matches!(tree_hash(d.path()), Err(PayloadError::NoRun)));
    std::fs::create_dir(d.path().join("run")).unwrap();
    assert!(matches!(tree_hash(d.path()), Err(PayloadError::NoRun)));
}

#[test]
fn a_missing_folder_is_an_io_error() {
    let d = tempfile::tempdir().unwrap();
    assert!(matches!(
        tree_hash(&d.path().join("none")),
        Err(PayloadError::Io(_))
    ));
}
