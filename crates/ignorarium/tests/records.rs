mod common;

use ed25519_dalek::{Signer, SigningKey};
use ignorarium::canon::{sha256_hex, to_canonical};
use ignorarium::events::{self, Event};
use ignorarium::home::{Home, is_windows_mount};
use ignorarium::keys::{self, KeyError};
use ignorarium::records::{self, Record, RecordDraft, VerifyError};
use ignorarium::time::rfc3339_from_unix_millis;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Barrier;

fn draft(n: u64) -> RecordDraft {
    RecordDraft {
        study: "s1".into(),
        row: Some(n),
        kind: "test".into(),
        body: json!({ "n": n }),
    }
}

fn key(b: u8) -> SigningKey {
    SigningKey::from_bytes(&[b; 32])
}

fn three(dir: &Path, k: &SigningKey) -> std::path::PathBuf {
    let p = dir.join("log.jsonl");
    for n in 0..3 {
        records::append(&p, k, draft(n)).unwrap();
    }
    p
}

fn lines(p: &Path) -> Vec<String> {
    std::fs::read_to_string(p)
        .unwrap()
        .lines()
        .map(String::from)
        .collect()
}

fn write_lines(p: &Path, v: &[String]) {
    std::fs::write(p, v.join("\n") + "\n").unwrap();
}

/// A stored line for `r`, signed by `k`.
fn stored(r: &Record, k: &SigningKey) -> String {
    let rv = serde_json::to_value(r).unwrap();
    let bytes = to_canonical(&rv).unwrap();
    let line = json!({
        "record": rv,
        "hash": sha256_hex(&bytes),
        "sig": hex::encode(k.sign(&bytes).to_bytes()),
    });
    String::from_utf8(to_canonical(&line).unwrap()).unwrap()
}

#[test]
fn append_three_verifies() {
    let d = tempfile::tempdir().unwrap();
    let k = key(1);
    let p = d.path().join("log.jsonl");
    let r0 = records::append(&p, &k, draft(0)).unwrap();
    let r1 = records::append(&p, &k, draft(1)).unwrap();
    records::append(&p, &k, draft(2)).unwrap();
    assert_eq!((r0.seq, r1.seq), (0, 1));
    assert_eq!(r0.prev, "0".repeat(64));
    let h0 = sha256_hex(&to_canonical(&serde_json::to_value(&r0).unwrap()).unwrap());
    assert_eq!(r1.prev, h0);
    assert_eq!(r0.ts.len(), 24);
    assert!(r0.ts.ends_with('Z'));
    assert_eq!(records::verify(&p, &k.verifying_key()).unwrap(), 3);
    assert!(std::fs::read(&p).unwrap().ends_with(b"\n"));
}

#[test]
fn missing_and_empty_log_verify_zero() {
    let d = tempfile::tempdir().unwrap();
    let k = key(1);
    let p = d.path().join("none.jsonl");
    assert_eq!(records::verify(&p, &k.verifying_key()).unwrap(), 0);
    std::fs::write(&p, b"").unwrap();
    assert_eq!(records::verify(&p, &k.verifying_key()).unwrap(), 0);
}

#[test]
fn edited_body_is_bad_hash() {
    let d = tempfile::tempdir().unwrap();
    let k = key(1);
    let p = three(d.path(), &k);
    let mut v = lines(&p);
    assert!(v[1].contains("\"n\":1}"));
    v[1] = v[1].replace("\"n\":1}", "\"n\":9}");
    write_lines(&p, &v);
    assert!(matches!(
        records::verify(&p, &k.verifying_key()),
        Err(VerifyError::BadHash { seq: 1 })
    ));
}

#[test]
fn swapped_lines_break_chain() {
    let d = tempfile::tempdir().unwrap();
    let k = key(1);
    let p = three(d.path(), &k);
    let mut v = lines(&p);
    v.swap(0, 1);
    write_lines(&p, &v);
    assert!(matches!(
        records::verify(&p, &k.verifying_key()),
        Err(VerifyError::BrokenChain { seq: 1 })
    ));
}

#[test]
fn deleted_line_breaks_chain() {
    let d = tempfile::tempdir().unwrap();
    let k = key(1);
    let p = three(d.path(), &k);
    let mut v = lines(&p);
    v.remove(1);
    write_lines(&p, &v);
    assert!(matches!(
        records::verify(&p, &k.verifying_key()),
        Err(VerifyError::BrokenChain { seq: 2 })
    ));
}

#[test]
fn wrong_prev_with_right_seq_breaks_chain() {
    let d = tempfile::tempdir().unwrap();
    let k = key(1);
    let p = three(d.path(), &k);
    let mut v = lines(&p);
    let line1: Value = serde_json::from_str(&v[1]).unwrap();
    let mut r: Record = serde_json::from_value(line1["record"].clone()).unwrap();
    r.prev = "f".repeat(64);
    v[1] = stored(&r, &k);
    write_lines(&p, &v);
    assert!(matches!(
        records::verify(&p, &k.verifying_key()),
        Err(VerifyError::BrokenChain { seq: 1 })
    ));
}

#[test]
fn wrong_key_is_bad_sig_at_first_record() {
    let d = tempfile::tempdir().unwrap();
    let p = three(d.path(), &key(1));
    assert!(matches!(
        records::verify(&p, &key(2).verifying_key()),
        Err(VerifyError::BadSig { seq: 0 })
    ));
}

#[test]
fn edited_seq_with_recomputed_hash_is_bad_sig() {
    let d = tempfile::tempdir().unwrap();
    let k = key(1);
    let p = three(d.path(), &k);
    let mut v = lines(&p);
    let mut line: Value = serde_json::from_str(&v[2]).unwrap();
    line["record"]["seq"] = json!(7);
    let bytes = to_canonical(&line["record"]).unwrap();
    line["hash"] = json!(sha256_hex(&bytes));
    v[2] = String::from_utf8(to_canonical(&line).unwrap()).unwrap();
    write_lines(&p, &v);
    assert!(matches!(
        records::verify(&p, &k.verifying_key()),
        Err(VerifyError::BadSig { seq: 7 })
    ));
}

#[test]
fn truncated_last_line_is_parse_error() {
    let d = tempfile::tempdir().unwrap();
    let k = key(1);
    let p = three(d.path(), &k);
    let bytes = std::fs::read(&p).unwrap();
    std::fs::write(&p, &bytes[..bytes.len() - 10]).unwrap();
    assert!(matches!(
        records::verify(&p, &k.verifying_key()),
        Err(VerifyError::Parse { line: 3 })
    ));
}

#[test]
fn complete_json_without_final_lf_is_parse_error() {
    let d = tempfile::tempdir().unwrap();
    let k = key(1);
    let p = three(d.path(), &k);
    let bytes = std::fs::read(&p).unwrap();
    std::fs::write(&p, &bytes[..bytes.len() - 1]).unwrap();
    assert!(matches!(
        records::verify(&p, &k.verifying_key()),
        Err(VerifyError::Parse { line: 3 })
    ));
}

#[test]
fn append_refuses_a_corrupt_tail() {
    let d = tempfile::tempdir().unwrap();
    let k = key(1);
    let p = three(d.path(), &k);
    let bytes = std::fs::read(&p).unwrap();
    std::fs::write(&p, &bytes[..bytes.len() - 10]).unwrap();
    assert!(records::append(&p, &k, draft(3)).is_err());
}

#[test]
fn two_threads_append_without_gaps() {
    let d = tempfile::tempdir().unwrap();
    let k = key(1);
    let p = d.path().join("log.jsonl");
    let gate = Barrier::new(2);
    std::thread::scope(|s| {
        for t in 0..2u64 {
            let (p, k, gate) = (&p, &k, &gate);
            s.spawn(move || {
                gate.wait();
                for n in 0..50 {
                    records::append(p, k, draft(t * 100 + n)).unwrap();
                }
            });
        }
    });
    assert_eq!(records::verify(&p, &k.verifying_key()).unwrap(), 100);
    let seqs: Vec<u64> = lines(&p)
        .iter()
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            v["record"]["seq"].as_u64().unwrap()
        })
        .collect();
    assert_eq!(seqs, (0..100).collect::<Vec<u64>>());
}

fn event(name: &str) -> Event {
    Event {
        ts: "2000-02-29T00:00:00.000Z".into(),
        trace_id: events::new_trace_id(),
        span_id: Some(events::new_span_id()),
        name: name.into(),
        level: "info".into(),
        attrs: json!({ "k": 1 }),
    }
}

#[cfg(unix)]
#[test]
fn append_and_emit_refuse_symlinked_log() {
    let d = tempfile::tempdir().unwrap();
    let real = d.path().join("real");
    std::fs::write(&real, b"").unwrap();
    let link = d.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(records::append(&link, &key(1), draft(0)).is_err());
    assert!(events::emit(&link, event("a")).is_err());
    assert_eq!(std::fs::read(&real).unwrap(), b"");
}

#[test]
fn emit_writes_one_json_line_with_trace_id() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("ev.jsonl");
    let e = event("one");
    let trace = e.trace_id.clone();
    events::emit(&p, e).unwrap();
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(text.ends_with('\n'));
    let v: Value = serde_json::from_str(text.trim_end()).unwrap();
    assert_eq!(v["trace_id"], json!(trace));
    assert_eq!(v["name"], "one");
    assert_eq!(v["level"], "info");
    assert_eq!(v["attrs"], json!({ "k": 1 }));
    assert_eq!(v["ts"], "2000-02-29T00:00:00.000Z");
}

#[test]
fn ids_are_lowercase_hex_of_fixed_length() {
    let (t, s) = (events::new_trace_id(), events::new_span_id());
    let ok = |x: &str| {
        x.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    assert_eq!((t.len(), s.len()), (32, 16));
    assert!(ok(&t) && ok(&s));
    assert_ne!(t, events::new_trace_id());
}

#[test]
fn emit_from_two_threads_keeps_every_line_whole() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("ev.jsonl");
    let gate = Barrier::new(2);
    std::thread::scope(|s| {
        for t in 0..2 {
            let (p, gate) = (&p, &gate);
            s.spawn(move || {
                gate.wait();
                for n in 0..50 {
                    events::emit(p, event(&format!("t{t}-{n}-{}", "x".repeat(2000)))).unwrap();
                }
            });
        }
    });
    let v = lines(&p);
    assert_eq!(v.len(), 100);
    for l in v {
        serde_json::from_str::<Value>(&l).unwrap();
    }
}

#[test]
fn rfc3339_known_vectors() {
    assert_eq!(rfc3339_from_unix_millis(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(
        rfc3339_from_unix_millis(951782400000),
        "2000-02-29T00:00:00.000Z"
    );
    assert_eq!(
        rfc3339_from_unix_millis(1700000000123),
        "2023-11-14T22:13:20.123Z"
    );
    assert_eq!(rfc3339_from_unix_millis(-1), "1969-12-31T23:59:59.999Z");
}

#[test]
fn windows_mount_detection() {
    assert!(is_windows_mount(Path::new("/mnt/c")));
    assert!(is_windows_mount(Path::new("/mnt/c/data/x")));
    assert!(is_windows_mount(Path::new("/mnt/d/x")));
    assert!(!is_windows_mount(Path::new("/mnt/cd/x")));
    assert!(!is_windows_mount(Path::new("/mnt")));
    assert!(!is_windows_mount(Path::new("/mnt/")));
    assert!(!is_windows_mount(Path::new("/mnt/1/x")));
    assert!(!is_windows_mount(Path::new("/srv/mnt/c")));
    assert!(!is_windows_mount(Path::new("mnt/c")));
}

#[test]
fn home_creates_subdirs_and_study_dir() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("box");
    let h = Home::open_at(&root).unwrap();
    for n in ["keys", "planlog", "testlog", "studies"] {
        assert!(root.join(n).is_dir(), "{n}");
    }
    assert_eq!(h.study_dir("abc"), root.join("studies").join("abc"));
    assert_eq!(h.root, root);
    Home::open_at(&root).unwrap();
}

#[cfg(unix)]
#[test]
fn home_dirs_are_0700_and_existing_are_tightened() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("box");
    std::fs::create_dir_all(root.join("keys")).unwrap();
    std::fs::set_permissions(root.join("keys"), std::fs::Permissions::from_mode(0o755)).unwrap();
    Home::open_at(&root).unwrap();
    for n in ["keys", "planlog", "testlog", "studies"] {
        let m = std::fs::metadata(root.join(n))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(m & 0o777, 0o700, "{n}");
    }
}

#[test]
fn key_is_created_once_and_reloaded() {
    let d = tempfile::tempdir().unwrap();
    let h = Home::open_at(d.path()).unwrap();
    let a = keys::load_or_create(&h).unwrap();
    let b = keys::load_or_create(&h).unwrap();
    assert_eq!(a.to_bytes(), b.to_bytes());
    let p = h.root.join("keys/box.ed25519");
    assert_eq!(std::fs::read(&p).unwrap().len(), 32);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(&p).unwrap();
        assert_eq!(m.permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn key_of_wrong_length_errors_and_is_kept() {
    let d = tempfile::tempdir().unwrap();
    let h = Home::open_at(d.path()).unwrap();
    let p = h.root.join("keys/box.ed25519");
    for bad in [&[7u8; 31][..], &[7u8; 33][..], &[][..]] {
        std::fs::write(&p, bad).unwrap();
        assert!(matches!(
            keys::load_or_create(&h),
            Err(KeyError::BadLength(n)) if n == bad.len()
        ));
        assert_eq!(std::fs::read(&p).unwrap(), bad);
    }
}

#[cfg(unix)]
#[test]
fn symlinked_key_errors() {
    let d = tempfile::tempdir().unwrap();
    let h = Home::open_at(d.path()).unwrap();
    let real = d.path().join("elsewhere");
    std::fs::write(&real, [1u8; 32]).unwrap();
    std::os::unix::fs::symlink(&real, h.root.join("keys/box.ed25519")).unwrap();
    assert!(matches!(keys::load_or_create(&h), Err(KeyError::Link)));
    assert!(matches!(keys::load(&h), Err(KeyError::Link)));
}

#[test]
fn load_reads_a_key_and_never_creates_one() {
    let d = tempfile::tempdir().unwrap();
    let h = Home::open_at(d.path()).unwrap();
    assert!(keys::load(&h).unwrap().is_none());
    assert!(!h.root.join("keys/box.ed25519").exists());
    let made = keys::load_or_create(&h).unwrap();
    assert_eq!(keys::load(&h).unwrap().unwrap().to_bytes(), made.to_bytes());
}

#[cfg(unix)]
#[test]
fn load_reports_an_unreadable_key_path() {
    let d = tempfile::tempdir().unwrap();
    let h = Home::open_at(d.path()).unwrap();
    std::fs::remove_dir(h.root.join("keys")).unwrap();
    std::fs::write(h.root.join("keys"), "x").unwrap();
    assert!(matches!(keys::load(&h), Err(KeyError::Io(_))));
}

#[test]
fn concurrent_key_creation_agrees() {
    let d = tempfile::tempdir().unwrap();
    let h = Home::open_at(d.path()).unwrap();
    let gate = Barrier::new(4);
    let got: Vec<[u8; 32]> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..4)
            .map(|_| {
                s.spawn(|| {
                    gate.wait();
                    keys::load_or_create(&h).unwrap().to_bytes()
                })
            })
            .collect();
        hs.into_iter().map(|j| j.join().unwrap()).collect()
    });
    assert!(got.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn emit_after_truncated_line_keeps_the_new_line_parseable() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("ev.jsonl");
    events::emit(&p, event("first")).unwrap();
    let bytes = std::fs::read(&p).unwrap();
    std::fs::write(&p, &bytes[..bytes.len() - 10]).unwrap();
    events::emit(&p, event("second")).unwrap();
    let v = lines(&p);
    assert_eq!(v.len(), 2);
    assert!(serde_json::from_str::<Value>(&v[0]).is_err());
    let last: Value = serde_json::from_str(&v[1]).unwrap();
    assert_eq!(last["name"], "second");
}

#[test]
fn append_refuses_complete_json_without_lf() {
    let d = tempfile::tempdir().unwrap();
    let k = key(1);
    let p = three(d.path(), &k);
    let bytes = std::fs::read(&p).unwrap();
    std::fs::write(&p, &bytes[..bytes.len() - 1]).unwrap();
    assert!(matches!(
        records::append(&p, &k, draft(3)),
        Err(records::RecordError::CorruptTail)
    ));
}

struct Counting<R> {
    inner: R,
    read: usize,
}
impl<R: std::io::Read> std::io::Read for Counting<R> {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(b)?;
        self.read += n;
        Ok(n)
    }
}
impl<R: std::io::Seek> std::io::Seek for Counting<R> {
    fn seek(&mut self, p: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(p)
    }
}

#[test]
fn tail_reader_reads_a_bounded_number_of_bytes() {
    use ignorarium::files::{Tail, read_tail};
    let mut data = Vec::new();
    for n in 0..1000 {
        data.extend_from_slice(
            format!(
                "{{\"n\":{n}}}{}
",
                " ".repeat(90)
            )
            .as_bytes(),
        );
    }
    let mut c = Counting {
        inner: std::io::Cursor::new(data.clone()),
        read: 0,
    };
    let t = read_tail(&mut c).unwrap();
    let want = format!(
        "{{\"n\":999}}{}
",
        " ".repeat(90)
    );
    assert!(matches!(&t, Tail::Line(l) if l == want.as_bytes()));
    assert!(c.read <= 2 * ignorarium::files::TAIL_CHUNK, "{}", c.read);
    assert!(data.len() > 10 * c.read);

    data.extend_from_slice(b"{\"n\":10");
    let t = read_tail(&mut std::io::Cursor::new(data)).unwrap();
    assert!(matches!(&t, Tail::Partial(l) if l == b"{\"n\":10"));
    assert!(matches!(
        read_tail(&mut std::io::Cursor::new(Vec::new())).unwrap(),
        Tail::Empty
    ));
    let one = read_tail(&mut std::io::Cursor::new(b"abc".to_vec())).unwrap();
    assert!(matches!(&one, Tail::Partial(l) if l == b"abc"));
    let one = read_tail(&mut std::io::Cursor::new(b"\n".to_vec())).unwrap();
    assert!(matches!(&one, Tail::Line(l) if l == b"\n"));
    let long = vec![b'x'; 3 * ignorarium::files::TAIL_CHUNK + 5];
    let mut with_lf = b"a\n".to_vec();
    with_lf.extend_from_slice(&long);
    with_lf.push(b'\n');
    let t = read_tail(&mut std::io::Cursor::new(with_lf)).unwrap();
    assert!(matches!(&t, Tail::Line(l) if l.len() == long.len() + 1));
}

#[test]
fn refuse_mount_cases() {
    use ignorarium::home::refuse_mount;
    // A relative path resolves against the working folder, so the child runs from `/`.
    const NAME: &str = "refuse_mount_cases";
    if !common::is_child(NAME) {
        assert!(common::in_child(NAME, Path::new("/"), &[]));
        return;
    }
    let wsl = "5.15.0-microsoft-standard-WSL2";
    let plain = "6.1.0-generic";
    assert!(refuse_mount(wsl, Path::new("/mnt/c/x")));
    assert!(!refuse_mount(plain, Path::new("/mnt/c/x")));
    assert!(refuse_mount("5.15.0-Microsoft", Path::new("/mnt/c/x")));
    assert!(refuse_mount(wsl, Path::new("/srv/x/../../mnt/c/y")));
    assert!(!refuse_mount(wsl, Path::new("/mnt/cdrom/x")));
    assert!(!refuse_mount(wsl, Path::new("/srv/data")));
    assert!(!refuse_mount(wsl, Path::new("relative/dir")));
    assert!(!refuse_mount(wsl, Path::new("/mnt/c/../d2/x")));
    assert!(refuse_mount(wsl, Path::new("/mnt/c/../d/x")));
}

#[cfg(unix)]
#[test]
fn home_refuses_symlinked_root_and_subdir() {
    let d = tempfile::tempdir().unwrap();
    let real = d.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = d.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(Home::open_at(&link).is_err());

    let root = d.path().join("box");
    std::fs::create_dir(&root).unwrap();
    std::os::unix::fs::symlink(&real, root.join("keys")).unwrap();
    assert!(Home::open_at(&root).is_err());
    assert_eq!(std::fs::read_dir(&real).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn home_root_is_created_0700() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("box");
    Home::open_at(&root).unwrap();
    let m = std::fs::metadata(&root).unwrap().permissions().mode();
    assert_eq!(m & 0o777, 0o700);
}

#[cfg(unix)]
#[test]
fn an_existing_home_root_keeps_its_mode_and_its_state_folders_are_tightened() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    Home::open_at(d.path()).unwrap();
    assert_eq!(mode(d.path()), 0o755);
    assert_eq!(mode(&d.path().join("keys")), 0o700);
}

#[cfg(unix)]
#[test]
fn an_existing_home_root_others_can_write_is_refused() {
    use ignorarium::home::HomeError;
    use std::os::unix::fs::PermissionsExt;
    for m in [0o775, 0o757, 0o1777] {
        let d = tempfile::tempdir().unwrap();
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(m)).unwrap();
        let e = Home::open_at(d.path()).unwrap_err();
        assert!(matches!(e, HomeError::Shared(_)), "{m:o}: {e}");
        assert!(!d.path().join("keys").exists(), "{m:o}");
    }
    let p = std::path::PathBuf::from("/srv/x");
    assert_eq!(
        HomeError::Shared(p).to_string(),
        "/srv/x is writable by other users; use a folder only you can write"
    );
}

#[test]
fn time_helper_does_not_panic_at_the_extremes() {
    assert_eq!(
        rfc3339_from_unix_millis(i64::MAX),
        "292278994-08-17T07:12:55.807Z"
    );
    assert_eq!(
        rfc3339_from_unix_millis(i64::MIN),
        "-292275055-05-16T16:47:04.192Z"
    );
    assert_eq!(
        rfc3339_from_unix_millis(253_402_300_800_000),
        "10000-01-01T00:00:00.000Z"
    );
}

#[test]
fn error_texts_name_their_cause() {
    use ignorarium::home::HomeError;
    let io = || std::io::Error::other("boom");
    let p = || std::path::PathBuf::from("/srv/x");
    for (got, want) in [
        (
            HomeError::NoHome.to_string(),
            "set IGNORARIUM_HOME: no home directory found",
        ),
        (
            HomeError::WindowsMount(p()).to_string(),
            "/srv/x is on a Windows drive; set IGNORARIUM_HOME to a Linux path",
        ),
        (
            HomeError::Link(p()).to_string(),
            "/srv/x is a symbolic link",
        ),
        (HomeError::Io(io()).to_string(), "I/O failed: boom"),
        (
            KeyError::BadLength(7).to_string(),
            "key file has 7 bytes, expected 32",
        ),
        (KeyError::Link.to_string(), "key path is a symbolic link"),
        (KeyError::Io(io()).to_string(), "I/O failed: boom"),
        (
            records::RecordError::CorruptTail.to_string(),
            "last line of the log is not a whole record",
        ),
        (
            records::RecordError::Io(io()).to_string(),
            "I/O failed: boom",
        ),
        (
            VerifyError::Parse { line: 4 }.to_string(),
            "line 4 is not a whole record",
        ),
        (
            VerifyError::BadHash { seq: 2 }.to_string(),
            "record 2: hash does not match",
        ),
        (
            VerifyError::BadSig { seq: 3 }.to_string(),
            "record 3: signature does not match",
        ),
        (
            VerifyError::BrokenChain { seq: 5 }.to_string(),
            "record 5: chain is broken",
        ),
        (VerifyError::Io(io()).to_string(), "I/O failed: boom"),
    ] {
        assert_eq!(got, want);
    }
    assert!(matches!(
        records::RecordError::from(io()),
        records::RecordError::Io(_)
    ));
    assert!(matches!(VerifyError::from(io()), VerifyError::Io(_)));
}

#[test]
fn verify_fails_on_a_log_path_it_cannot_read() {
    let d = tempfile::tempdir().unwrap();
    // A directory exists but cannot be read as a file: not the same as an absent log.
    assert!(matches!(
        records::verify(d.path(), &key(1).verifying_key()),
        Err(VerifyError::Io(_))
    ));
}

#[test]
fn timestamps_match_a_day_by_day_calendar() {
    const DAY: i64 = 86_400_000;
    let leap = |y: i64| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let (mut y, mut m, mut d) = (1600i64, 1i64, 1i64);
    let mut days = -135_140; // 1600-01-01, in days from 1970-01-01
    while y < 2500 {
        let want = format!("{y:04}-{m:02}-{d:02}T00:00:00.000Z");
        assert_eq!(rfc3339_from_unix_millis(days * DAY), want);
        let len = match m {
            2 if leap(y) => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        days += 1;
        d += 1;
        if d > len {
            (d, m) = (1, m + 1);
        }
        if m > 12 {
            (m, y) = (1, y + 1);
        }
    }
}

#[test]
fn a_record_with_long_floats_verifies_after_a_write_and_read() {
    let d = tempfile::tempdir().unwrap();
    let k = key(1);
    let p = d.path().join("log.jsonl");
    let body = |floats: Vec<f64>| RecordDraft {
        body: json!({ "m": floats }),
        ..draft(0)
    };
    records::append(&p, &k, body(vec![0.1 + 0.2, 1.0 / 3.0, 12345.678901234567])).unwrap();
    // 200 pseudo-random 17-digit floats: the default float parser misreads some of them.
    let mut bits: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut many = Vec::new();
    for _ in 0..200 {
        bits = bits
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        many.push(f64::from_bits(0x3FF0_0000_0000_0000 | (bits >> 12)));
    }
    records::append(&p, &k, body(many)).unwrap();
    assert_eq!(records::verify(&p, &k.verifying_key()).unwrap(), 2);
}

/// A box home with one record in each of the studies `a` and `b`.
fn two_studies(k: &SigningKey) -> (tempfile::TempDir, Home, Record, Record) {
    let d = tempfile::tempdir().unwrap();
    let home = Home::open_at(&d.path().join("home")).unwrap();
    let mut made = Vec::new();
    for (study, n) in [("a", 1), ("b", 2)] {
        std::fs::create_dir(home.study_dir(study)).unwrap();
        let p = home.study_dir(study).join("records.jsonl");
        made.push(records::append(&p, k, draft(n)).unwrap());
    }
    let (second, first) = (made.pop().unwrap(), made.pop().unwrap());
    (d, home, first, second)
}

#[test]
fn hash_equals_the_stored_hash() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("log.jsonl");
    let r = records::append(&p, &key(1), draft(0)).unwrap();
    let line: Value = serde_json::from_str(&lines(&p)[0]).unwrap();
    assert_eq!(records::hash(&r), line["hash"].as_str().unwrap());
    assert_eq!(records::hash(&r).len(), 64);
}

#[test]
fn find_returns_the_record_with_that_hash_from_any_study() {
    let k = key(1);
    let (_d, home, first, second) = two_studies(&k);
    let v = k.verifying_key();
    assert_eq!(
        records::find(&home, &v, &records::hash(&second)).unwrap(),
        Some(second)
    );
    assert_eq!(
        records::find(&home, &v, &records::hash(&first)).unwrap(),
        Some(first)
    );
}

#[test]
fn find_returns_none_for_an_unknown_hash() {
    let k = key(1);
    let (_d, home, ..) = two_studies(&k);
    assert_eq!(
        records::find(&home, &k.verifying_key(), &"0".repeat(64)).unwrap(),
        None
    );
}

#[test]
fn find_fails_on_a_log_that_does_not_verify() {
    let k = key(1);
    let (_d, home, first, _) = two_studies(&k);
    // The wanted record sits in `a`; the damaged log `b` is read all the same.
    let p = home.study_dir("b").join("records.jsonl");
    let damaged = std::fs::read_to_string(&p)
        .unwrap()
        .replace("\"n\":2", "\"n\":9");
    std::fs::write(&p, damaged).unwrap();
    let err = records::find(&home, &k.verifying_key(), &records::hash(&first)).unwrap_err();
    assert!(matches!(err, VerifyError::BadHash { seq: 0 }), "{err}");
}
