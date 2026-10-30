//! The input view: one hard link per declared input, reused only when it matches.
mod common;

use ignorarium::store::Store;
use ignorarium::view::{self, ViewError};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Bench {
    dir: TempDir,
    store: Store,
    study: PathBuf,
}

impl Drop for Bench {
    fn drop(&mut self) {
        common::unseal(self.dir.path());
    }
}

fn bench() -> Bench {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let study = dir.path().join("studies/s");
    std::fs::create_dir_all(&study).unwrap();
    Bench { dir, store, study }
}

/// Import `text` as a stored blob and return its digest.
fn stored(b: &Bench, name: &str, text: &str) -> String {
    let f = b.dir.path().join(name);
    std::fs::write(&f, text).unwrap();
    b.store.import(&f).unwrap().0
}

/// Every path below `dir`, sorted, folders included.
fn listing(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            out.push(p.strip_prefix(dir).unwrap().display().to_string());
            if p.symlink_metadata().unwrap().is_dir() {
                todo.push(p);
            }
        }
    }
    out.sort();
    out
}

fn inode(p: &Path) -> (u64, u64) {
    let m = p.symlink_metadata().unwrap();
    (m.dev(), m.ino())
}

fn mode(p: &Path) -> u32 {
    p.symlink_metadata().unwrap().permissions().mode() & 0o777
}

#[test]
fn a_matching_view_is_reused_and_a_different_one_moved_aside() {
    let b = bench();
    let a = stored(&b, "a.txt", "alpha");
    let m = stored(&b, "m.bin", "model");
    let both = [
        ("data/a.txt".to_string(), a.clone()),
        ("models/m.bin".to_string(), m.clone()),
    ];
    let dir = view::build(&b.store, &b.study, &both).unwrap();
    assert_eq!(dir, b.study.join("in"));
    assert!(dir.is_absolute());
    assert_eq!(
        listing(&dir),
        ["data", "data/a.txt", "models", "models/m.bin"]
    );
    // Each entry is the stored blob itself, and no folder takes a new entry.
    assert_eq!(
        inode(&dir.join("data/a.txt")),
        inode(&b.store.path(&a).unwrap())
    );
    assert_eq!(
        inode(&dir.join("models/m.bin")),
        inode(&b.store.path(&m).unwrap())
    );
    for f in ["", "data", "models"] {
        assert_eq!(mode(&dir.join(f)), 0o555, "{f:?}");
    }
    // The same inputs again: the folder is kept as it is.
    let first = inode(&dir);
    assert_eq!(view::build(&b.store, &b.study, &both).unwrap(), dir);
    assert_eq!(inode(&dir), first);
    assert!(!b.study.join("in.old-0").exists());
    // Other inputs: the old view moves aside whole, and a new one holds exactly these.
    let one = [("data/a.txt".to_string(), a.clone())];
    view::build(&b.store, &b.study, &one).unwrap();
    assert_eq!(inode(&b.study.join("in.old-0")), first);
    assert_eq!(
        listing(&b.study.join("in.old-0")),
        ["data", "data/a.txt", "models", "models/m.bin"]
    );
    assert_eq!(listing(&dir), ["data", "data/a.txt"]);
    // A file that is a copy, not the blob, also moves the view aside: to the next free name.
    std::fs::set_permissions(dir.join("data"), PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::rename(dir.join("data/a.txt"), b.dir.path().join("link.old")).unwrap();
    std::fs::write(dir.join("data/a.txt"), "alpha").unwrap();
    view::build(&b.store, &b.study, &one).unwrap();
    assert!(b.study.join("in.old-1/data/a.txt").is_file());
    assert_eq!(
        inode(&dir.join("data/a.txt")),
        inode(&b.store.path(&a).unwrap())
    );
    // An extra entry is not a match either.
    std::fs::set_permissions(&dir, PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::write(dir.join("extra"), "").unwrap();
    view::build(&b.store, &b.study, &one).unwrap();
    assert!(b.study.join("in.old-2/extra").is_file());
    assert_eq!(listing(&dir), ["data", "data/a.txt"]);
    // No inputs: an empty view.
    view::build(&b.store, &b.study, &[]).unwrap();
    assert!(listing(&dir).is_empty());
    assert!(b.study.join("in.old-3/data/a.txt").is_file());
}

#[test]
fn a_view_needs_every_blob_in_the_store() {
    let b = bench();
    let a = stored(&b, "a.txt", "alpha");
    let gone = "0".repeat(64);
    let inputs = [
        ("data/a.txt".to_string(), a),
        ("data/b.txt".to_string(), gone),
    ];
    match view::build(&b.store, &b.study, &inputs) {
        Err(ViewError::Missing(p)) => assert_eq!(p, "data/b.txt"),
        other => panic!("{other:?}"),
    }
    assert!(!b.study.join("in").exists());
}

#[test]
fn verify_hashes_each_file_and_names_one_that_changed() {
    let b = bench();
    let a = stored(&b, "a.txt", "alpha");
    let inputs = [("data/a.txt".to_string(), a.clone())];
    let dir = view::build(&b.store, &b.study, &inputs).unwrap();
    let seen = view::verify(&dir, &inputs).unwrap();
    assert_eq!(seen, view::scan(&dir).unwrap());
    assert_eq!(seen.len(), 3, "{seen:?}");
    // Same size, other bytes: only a full hash sees it.
    let blob = b.store.path(&a).unwrap();
    std::fs::set_permissions(&blob, PermissionsExt::from_mode(0o644)).unwrap();
    std::fs::write(&blob, "alphA").unwrap();
    match view::verify(&dir, &inputs) {
        Err(ViewError::Changed(p)) => assert_eq!(p, "data/a.txt"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_scan_sees_a_new_size_but_not_a_new_mode() {
    let b = bench();
    let a = stored(&b, "a.txt", "alpha");
    let inputs = [("data/a.txt".to_string(), a.clone())];
    let dir = view::build(&b.store, &b.study, &inputs).unwrap();
    let seen = view::scan(&dir).unwrap();
    assert_eq!(view::scan(&dir).unwrap(), seen);
    let blob = b.store.path(&a).unwrap();
    std::fs::set_permissions(&blob, PermissionsExt::from_mode(0o644)).unwrap();
    // A changed mode alone is not a change of content.
    assert_eq!(view::scan(&dir).unwrap(), seen);
    let key = view::key("data/a.txt");
    assert_eq!(key, view::key("data//a.txt"));
    std::fs::OpenOptions::new()
        .append(true)
        .open(&blob)
        .unwrap()
        .set_len(6)
        .unwrap();
    let grown = view::scan(&dir).unwrap();
    assert_ne!(grown.get(&key), seen.get(&key));
    // A missing view scans as an error.
    let other = b.study.join("none");
    assert!(view::scan(&other).is_err());
}
