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

#[test]
fn put_reader_failure_leaves_no_blob_or_temp() {
    struct Bad(bool);
    impl std::io::Read for Bad {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            if self.0 {
                return Err(std::io::Error::other("cut"));
            }
            self.0 = true;
            b[0] = 1;
            Ok(1)
        }
    }
    let (_d, s) = open();
    assert!(matches!(
        s.put_reader(&mut Bad(false)),
        Err(StoreError::Io(_))
    ));
    assert!(leftovers(&s).is_empty());
    assert_eq!(std::fs::read_dir(&s.root).unwrap().count(), 0);
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
