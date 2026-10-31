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
fn a_name_that_is_not_utf8_moves_the_view_aside() {
    use std::os::unix::ffi::OsStrExt;
    let b = bench();
    let a = stored(&b, "a.txt", "alpha");
    let inputs = [("data/a.txt".to_string(), a)];
    let dir = view::build(&b.store, &b.study, &inputs).unwrap();
    std::fs::set_permissions(&dir, PermissionsExt::from_mode(0o755)).unwrap();
    let odd = dir.join(std::ffi::OsStr::from_bytes(b"\xff"));
    std::fs::write(&odd, "").unwrap();
    view::build(&b.store, &b.study, &inputs).unwrap();
    assert!(
        b.study
            .join("in.old-0")
            .join(odd.file_name().unwrap())
            .is_file()
    );
    assert_eq!(listing(&dir), ["data", "data/a.txt"]);
}

/// File times come from a clock that ticks every few milliseconds: wait past one tick, so
/// the next change moves them.
fn tick() {
    std::thread::sleep(std::time::Duration::from_millis(30));
}

/// Set the modification time of `p`, which takes write access.
fn set_mtime(p: &Path, t: std::time::SystemTime) {
    std::fs::set_permissions(p, PermissionsExt::from_mode(0o644)).unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
    f.set_modified(t).unwrap();
}

#[test]
fn changes_name_what_moved_and_hash_a_file_whose_ctime_alone_moved() {
    let b = bench();
    let a = stored(&b, "a.txt", "alpha");
    // Empty segments in an input path name the same file.
    let inputs = [("data//a.txt".to_string(), a.clone())];
    let dir = view::build(&b.store, &b.study, &inputs).unwrap();
    let seen = view::verify(&dir, &inputs).unwrap();
    let blob = b.store.path(&a).unwrap();
    let none: &[String] = &[];
    let c = view::changes(&dir, &inputs, &seen).unwrap();
    assert_eq!((c.inputs.as_slice(), c.other), (none, false));
    assert_eq!(c.now, seen);
    // A file whose stamp did not move is not read: a digest it would not match goes unseen.
    let unread = [("data//a.txt".to_string(), "0".repeat(64))];
    assert_eq!(view::changes(&dir, &unread, &seen).unwrap().inputs, none);
    // A mode change moves the ctime alone: the file is hashed and its bytes are the same.
    tick();
    std::fs::set_permissions(&blob, PermissionsExt::from_mode(0o644)).unwrap();
    let c = view::changes(&dir, &inputs, &seen).unwrap();
    assert_eq!((c.inputs.as_slice(), c.other), (none, false));
    assert_ne!(c.now, seen);
    // A new modification time counts without a hash, even over the same bytes.
    let seen = c.now;
    let then = blob.metadata().unwrap().modified().unwrap();
    set_mtime(&blob, then + std::time::Duration::from_secs(10));
    let c = view::changes(&dir, &inputs, &seen).unwrap();
    assert_eq!(c.inputs, ["data//a.txt"]);
    assert!(!c.other);
    // Same size, other bytes, the old time put back: only the ctime moved, and the hash
    // differs.
    let seen = c.now;
    let then = blob.metadata().unwrap().modified().unwrap();
    tick();
    std::fs::write(&blob, "alphA").unwrap();
    set_mtime(&blob, then);
    let c = view::changes(&dir, &inputs, &seen).unwrap();
    assert_eq!(c.inputs, ["data//a.txt"]);
    // A file that is no longer the blob counts, even with the same bytes and time.
    let fresh = bench();
    let a = stored(&fresh, "a.txt", "alpha");
    let inputs = [("data/a.txt".to_string(), a)];
    let dir = view::build(&fresh.store, &fresh.study, &inputs).unwrap();
    let seen = view::verify(&dir, &inputs).unwrap();
    let file = dir.join("data/a.txt");
    let then = file.metadata().unwrap().modified().unwrap();
    std::fs::set_permissions(dir.join("data"), PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::rename(&file, fresh.dir.path().join("old-link")).unwrap();
    std::fs::write(&file, "alpha").unwrap();
    set_mtime(&file, then);
    let c = view::changes(&dir, &inputs, &seen).unwrap();
    assert_eq!(c.inputs, ["data/a.txt"]);
    // An entry that is not an input file counts as the view's own change.
    let seen = c.now;
    std::fs::set_permissions(&dir, PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::write(dir.join("extra"), "").unwrap();
    let c = view::changes(&dir, &inputs, &seen).unwrap();
    assert_eq!((c.inputs.as_slice(), c.other), (none, true));
    // A missing view is an error.
    assert!(view::changes(&fresh.study.join("none"), &inputs, &seen).is_err());
}

#[test]
fn view_errors_name_the_input() {
    let shown = |e: ViewError| e.to_string();
    assert_eq!(
        shown(ViewError::Missing("data/a.txt".into())),
        "the store has no blob for input data/a.txt"
    );
    assert_eq!(
        shown(ViewError::Changed("data/a.txt".into())),
        "input data/a.txt no longer hashes to its digest"
    );
    assert_eq!(
        shown(ViewError::Unsafe("../a".into())),
        "input path ../a could leave the view"
    );
    let io = std::io::Error::other("disk gone");
    assert_eq!(shown(ViewError::from(io)), "disk gone");
}
