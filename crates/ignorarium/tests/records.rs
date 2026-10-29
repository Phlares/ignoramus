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
