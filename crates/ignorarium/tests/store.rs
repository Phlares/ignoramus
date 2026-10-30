use ignorarium::store::{Store, StoreError};
use std::io::Cursor;

const HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

fn open() -> (tempfile::TempDir, Store) {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    (d, s)
}

fn blob() -> Vec<u8> {
    (0..100 * 1024u32).map(|i| (i % 251) as u8).collect()
}

fn leftovers(s: &Store) -> Vec<String> {
    std::fs::read_dir(&s.root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.'))
        .collect()
}

#[test]
fn put_is_idempotent_and_addresses_by_hash() {
    let (_d, s) = open();
    let h = s.put(b"hello").unwrap();
    assert_eq!(h, HELLO);
    assert_eq!(s.put(b"hello").unwrap(), h);
    assert!(s.has(&h));
    assert_eq!(std::fs::read(s.path(&h).unwrap()).unwrap(), b"hello");
}

#[test]
fn has_is_false_for_an_absent_digest() {
    let (_d, s) = open();
    assert!(!s.has(HELLO));
}

#[test]
fn put_reader_matches_put() {
    let (_d, s) = open();
    let bytes = blob();
    let streamed = s.put_reader(&mut Cursor::new(&bytes)).unwrap();
    assert_eq!(streamed.len(), 64);
    assert_eq!(s.put(&bytes).unwrap(), streamed);
    assert_eq!(std::fs::read(s.path(&streamed).unwrap()).unwrap(), bytes);
    assert_eq!(s.put_reader(&mut Cursor::new(&bytes)).unwrap(), streamed);
    assert!(leftovers(&s).is_empty());
}

#[test]
fn put_reader_handles_an_empty_stream() {
    let (_d, s) = open();
    let h = s.put_reader(&mut Cursor::new(Vec::new())).unwrap();
    assert_eq!(
        h,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert!(s.has(&h));
}

/// Gives one byte, then each scripted error once, then the end of the stream.
struct Flaky(Vec<std::io::ErrorKind>, bool);

impl std::io::Read for Flaky {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        if !self.1 {
            self.1 = true;
            b[0] = 1;
            return Ok(1);
        }
        match self.0.pop() {
            Some(kind) => Err(std::io::Error::new(kind, "cut")),
            None => Ok(0),
        }
    }
}

#[test]
fn put_reader_failure_leaves_no_blob_or_temp() {
    let (_d, s) = open();
    let mut bad = Flaky(vec![std::io::ErrorKind::Other], false);
    assert!(matches!(s.put_reader(&mut bad), Err(StoreError::Io(_))));
    assert!(leftovers(&s).is_empty());
    assert_eq!(std::fs::read_dir(&s.root).unwrap().count(), 0);
}

#[test]
fn put_reader_retries_an_interrupted_read() {
    let (_d, s) = open();
    let interrupted = std::io::ErrorKind::Interrupted;
    let h = s.put_reader(&mut Flaky(vec![interrupted, interrupted], false));
    assert_eq!(h.unwrap(), s.put(&[1]).unwrap());
}

#[test]
fn store_errors_read_as_plain_text() {
    assert_eq!(
        StoreError::BadDigest.to_string(),
        "digest is not 64 lowercase hex characters"
    );
    assert_eq!(
        StoreError::Link.to_string(),
        "store path is a symbolic link"
    );
    assert_eq!(
        StoreError::Io(std::io::Error::other("gone")).to_string(),
        "I/O failed: gone"
    );
}

#[cfg(unix)]
#[test]
fn blobs_are_read_only() {
    use std::os::unix::fs::PermissionsExt;
    let (_d, s) = open();
    let h = s.put(b"hello").unwrap();
    let m = std::fs::metadata(s.path(&h).unwrap()).unwrap();
    assert_eq!(m.permissions().mode() & 0o777, 0o444);
    let h2 = s.put_reader(&mut Cursor::new(b"other")).unwrap();
    let m = std::fs::metadata(s.path(&h2).unwrap()).unwrap();
    assert_eq!(m.permissions().mode() & 0o777, 0o444);
}

#[cfg(unix)]
#[test]
fn store_dir_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let (_d, s) = open();
    let m = std::fs::metadata(&s.root).unwrap();
    assert_eq!(m.permissions().mode() & 0o777, 0o700);
}

#[cfg(unix)]
#[test]
fn store_refuses_a_symlinked_root() {
    let d = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), d.path().join("store")).unwrap();
    assert!(matches!(Store::open(d.path()), Err(StoreError::Link)));
}

#[cfg(unix)]
#[test]
fn a_symlink_at_the_digest_path_is_refused() {
    let (d, s) = open();
    let target = d.path().join("target");
    std::fs::write(&target, b"x").unwrap();
    std::os::unix::fs::symlink(&target, s.root.join(HELLO)).unwrap();
    assert!(matches!(s.put(b"hello"), Err(StoreError::Link)));
    assert!(matches!(
        s.put_reader(&mut Cursor::new(b"hello")),
        Err(StoreError::Link)
    ));
    assert_eq!(std::fs::read(&target).unwrap(), b"x");
}

#[test]
fn bad_digests_are_refused() {
    let (_d, s) = open();
    let upper = HELLO.to_uppercase();
    let short = &HELLO[..63];
    let long = format!("{HELLO}0");
    let dots = format!("../{}", &HELLO[3..]);
    let nonhex = format!("g{}", &HELLO[1..]);
    for bad in [
        "",
        "abc",
        upper.as_str(),
        short,
        long.as_str(),
        dots.as_str(),
        nonhex.as_str(),
    ] {
        assert!(matches!(s.path(bad), Err(StoreError::BadDigest)), "{bad}");
        assert!(!s.has(bad), "{bad}");
    }
    assert!(s.path(HELLO).is_ok());
}

// ---- import ----

use ignorarium::canon::sha256_hex;
use ignorarium::prior::{file_hashes, file_hashes_within, normalized_sha256};
use ignorarium::store::{Meta, NORMALIZE_LIMIT};

fn source(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("in.bin");
    std::fs::write(&f, bytes).unwrap();
    (d, f)
}

fn meta_file(s: &Store, digest: &str) -> std::path::PathBuf {
    s.root.join("meta").join(format!("{digest}.json"))
}

#[test]
fn import_streams_the_blob_and_writes_meta_once() {
    let (_d, s) = open();
    let (_src, f) = source(b"Hello  World");
    let (digest, meta) = s.import(&f).unwrap();
    assert_eq!(digest, sha256_hex(b"Hello  World"));
    assert_eq!(
        meta,
        Meta {
            size: 12,
            normalized_sha256: normalized_sha256("hello world")
        }
    );
    assert_eq!(
        std::fs::read(s.path(&digest).unwrap()).unwrap(),
        b"Hello  World"
    );
    assert_eq!(s.meta(&digest).unwrap(), Some(meta.clone()));
    let first = std::fs::read(meta_file(&s, &digest)).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&first).unwrap(),
        serde_json::json!({"size": 12, "normalized_sha256": meta.normalized_sha256})
    );
    // A second import changes nothing.
    assert_eq!(s.import(&f).unwrap(), (digest.clone(), meta));
    assert_eq!(std::fs::read(meta_file(&s, &digest)).unwrap(), first);
    assert!(leftovers(&s).is_empty());
}

#[test]
fn meta_is_none_for_an_unknown_digest_and_refuses_a_bad_one() {
    let (_d, s) = open();
    assert_eq!(s.meta(HELLO).unwrap(), None);
    assert!(matches!(s.meta("../x"), Err(StoreError::BadDigest)));
}

#[test]
fn a_changed_meta_file_is_an_error_not_a_silent_rewrite() {
    let (_d, s) = open();
    let (_src, f) = source(b"abc");
    let (digest, _) = s.import(&f).unwrap();
    let path = meta_file(&s, &digest);
    let mut perm = std::fs::metadata(&path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    std::fs::set_permissions(&path, perm).unwrap();
    std::fs::write(&path, b"{\"size\":1,\"normalized_sha256\":\"\"}").unwrap();
    assert!(s.import(&f).is_err());
}

#[test]
fn a_binary_file_has_no_normalized_hash() {
    let (_d, s) = open();
    let (_src, f) = source(&[0xff, 0xfe, 0x00, 0x01]);
    let (_, meta) = s.import(&f).unwrap();
    assert_eq!(meta.size, 4);
    assert_eq!(meta.normalized_sha256, "");
}

#[test]
fn the_limit_is_64_mib_and_the_plain_calls_use_it() {
    assert_eq!(NORMALIZE_LIMIT, 64 << 20);
    let (_d, s) = open();
    let (_src, f) = source(b"Some  Text");
    assert_eq!(
        s.import(&f).unwrap().1.normalized_sha256,
        file_hashes(b"Some  Text").1
    );
    assert_eq!(
        file_hashes_within(b"Some  Text", NORMALIZE_LIMIT),
        file_hashes(b"Some  Text")
    );
}

#[test]
fn the_normalized_hash_stops_at_the_size_limit() {
    let (at, over) = (b"Ab Cd  e", b"Ab Cd  eF");
    assert_eq!(file_hashes_within(at, 8).1, normalized_sha256("ab cd e"));
    assert_eq!(file_hashes_within(over, 8).1, "");
    assert_eq!(file_hashes_within(over, 8).0, sha256_hex(over));
    let (_d, s) = open();
    let (_src, f) = source(at);
    let (_, meta) = s.import_within(&f, 8).unwrap();
    assert_eq!(
        (meta.size, meta.normalized_sha256),
        (8, normalized_sha256("ab cd e"))
    );
    let (_src2, f2) = source(over);
    let (_, meta) = s.import_within(&f2, 8).unwrap();
    assert_eq!((meta.size, meta.normalized_sha256), (9, String::new()));
}

#[test]
fn re_importing_over_a_corrupt_blob_is_refused_and_writes_no_meta() {
    let (_d, s) = open();
    let (_src, f) = source(b"abc");
    let digest = sha256_hex(b"abc");
    std::fs::write(s.root.join(&digest), b"bad").unwrap();
    assert!(s.import(&f).is_err());
    assert!(!meta_file(&s, &digest).exists());
}

#[test]
fn re_importing_over_an_intact_blob_succeeds() {
    let (_d, s) = open();
    let (_src, f) = source(b"abc");
    s.put(b"abc").unwrap();
    assert_eq!(s.import(&f).unwrap().0, sha256_hex(b"abc"));
}

#[test]
fn import_refuses_a_folder_and_a_missing_file() {
    let (_d, s) = open();
    let (src, _) = source(b"x");
    assert!(matches!(s.import(src.path()), Err(StoreError::NotRegular)));
    assert!(matches!(
        s.import(&src.path().join("none")),
        Err(StoreError::Io(_))
    ));
}

#[cfg(unix)]
#[test]
fn import_refuses_a_link() {
    let (_d, s) = open();
    let (src, f) = source(b"x");
    let link = src.path().join("link");
    std::os::unix::fs::symlink(&f, &link).unwrap();
    assert!(matches!(s.import(&link), Err(StoreError::Link)));
    assert!(!s.has(&sha256_hex(b"x")));
    assert!(!s.root.join("meta").exists());
}
