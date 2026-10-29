use bundle::manifest::sha256_bytes;
use bundle::zip::{crc32, pack};
use bundle::{Error, Problem};
use std::fs::{self, File};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

const PIN: &str = "d0e6806e42bbf319afd83d43380f369a71804d384a560dfeb9cd224b84800f7f";

fn study(dir: &Path) {
    fs::write(dir.join("a.txt"), "hi\n").unwrap();
    fs::create_dir(dir.join("d")).unwrap();
    fs::write(dir.join("d/b.txt"), "").unwrap();
}

fn refused(e: Error) -> Vec<Problem> {
    match e {
        Error::Refused { problems, .. } => problems,
        other => panic!("expected Refused, got {other:?}"),
    }
}

#[test]
fn crc32_matches_the_check_value() {
    assert_eq!(crc32(b"123456789"), 0xCBF43926);
    assert_eq!(crc32(b""), 0);
}

#[test]
fn pack_writes_the_reference_bytes() {
    let t = tempfile::tempdir().unwrap();
    let o = tempfile::tempdir().unwrap();
    study(t.path());
    let out = o.path().join("b.zip");
    let hash = pack(t.path(), &out).unwrap();
    let bytes = fs::read(&out).unwrap();
    assert_eq!(bytes.len(), 201);
    assert_eq!(hash, PIN);
    assert_eq!(hash, sha256_bytes(&bytes));
}

#[test]
fn pack_ignores_times_and_modes() {
    let t = tempfile::tempdir().unwrap();
    let o = tempfile::tempdir().unwrap();
    study(t.path());
    let f = File::options()
        .write(true)
        .open(t.path().join("a.txt"))
        .unwrap();
    f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(978_307_200))
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(t.path().join("a.txt"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert_eq!(pack(t.path(), &o.path().join("b.zip")).unwrap(), PIN);
}

#[test]
fn pack_refuses_an_existing_out() {
    let t = tempfile::tempdir().unwrap();
    let o = tempfile::tempdir().unwrap();
    study(t.path());
    let out = o.path().join("b.zip");
    fs::write(&out, "keep").unwrap();
    match pack(t.path(), &out) {
        Err(Error::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists),
        other => panic!("expected Io, got {other:?}"),
    }
    assert_eq!(fs::read_to_string(&out).unwrap(), "keep");
}

#[test]
fn pack_refuses_four_gib_without_reading() {
    let t = tempfile::tempdir().unwrap();
    let o = tempfile::tempdir().unwrap();
    study(t.path());
    File::create(t.path().join("big.bin"))
        .unwrap()
        .set_len(0x1_0000_0000)
        .unwrap();
    let out = o.path().join("b.zip");
    let start = Instant::now();
    let problems = refused(pack(t.path(), &out).unwrap_err());
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].code, "E-PUB-012");
    assert_eq!(problems[0].path, "big.bin");
    assert_eq!(problems[0].message, "too large for a bundle zip");
    assert_eq!(
        problems[0].instruction,
        "Deposit files of 4 GiB or more as artifacts beside the bundle; a bundle zip holds at most 65,535 files under 4 GiB in all."
    );
    assert!(!out.exists());
}

#[test]
fn pack_refuses_a_total_of_four_gib() {
    let t = tempfile::tempdir().unwrap();
    let o = tempfile::tempdir().unwrap();
    for n in ["a.bin", "b.bin"] {
        File::create(t.path().join(n))
            .unwrap()
            .set_len(0x8000_0000)
            .unwrap();
    }
    let problems = refused(pack(t.path(), &o.path().join("b.zip")).unwrap_err());
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].code, "E-PUB-012");
    assert_eq!(problems[0].path, "");
    assert_eq!(problems[0].message, "bundle zip would reach 4 GiB");
}

#[test]
fn pack_refuses_unsafe_entries() {
    let t = tempfile::tempdir().unwrap();
    let o = tempfile::tempdir().unwrap();
    study(t.path());
    #[cfg(unix)]
    std::os::unix::fs::symlink("a.txt", t.path().join("link")).unwrap();
    let out = o.path().join("b.zip");
    let problems = refused(pack(t.path(), &out).unwrap_err());
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].code, "E-PUB-001");
    assert_eq!(problems[0].path, "link");
    assert!(!out.exists());
}

#[test]
fn pack_files_refuses_a_file_changed_after_the_walk() {
    let t = tempfile::tempdir().unwrap();
    let o = tempfile::tempdir().unwrap();
    study(t.path());
    let (files, _) = bundle::tree::walk(t.path()).unwrap();
    fs::write(t.path().join("a.txt"), "hi there\n").unwrap();
    let out = o.path().join("b.zip");
    match bundle::zip::pack_files(t.path(), &files, &out) {
        Err(Error::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidData),
        other => panic!("expected Io, got {other:?}"),
    }
}
