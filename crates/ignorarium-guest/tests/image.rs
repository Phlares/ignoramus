use ignorarium_guest::image::{Pin, check_pin, ext4, extract, initramfs, sha256_file, size_mib};
use ignorarium_guest::tree::tree_hash;
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::Path;

struct Entry {
    name: String,
    mode: u32,
    nlink: u32,
    uid: u32,
    gid: u32,
    mtime: u32,
    rdev: (u32, u32),
    data: Vec<u8>,
}

fn field(b: &[u8], n: usize) -> u32 {
    let at = 6 + 8 * n;
    u32::from_str_radix(std::str::from_utf8(&b[at..at + 8]).unwrap(), 16).unwrap()
}

fn parse_newc(mut b: &[u8]) -> Vec<Entry> {
    let mut out = Vec::new();
    while !b.is_empty() {
        assert_eq!(&b[..6], b"070701");
        let size = field(b, 6) as usize;
        let namesize = field(b, 11) as usize;
        let name = String::from_utf8(b[110..110 + namesize - 1].to_vec()).unwrap();
        assert_eq!(b[110 + namesize - 1], 0);
        let data_at = (110 + namesize).next_multiple_of(4);
        let end = (data_at + size).next_multiple_of(4);
        assert!(b.len() >= end, "entry {name} is not padded");
        out.push(Entry {
            name,
            mode: field(b, 1),
            nlink: field(b, 4),
            uid: field(b, 2),
            gid: field(b, 3),
            mtime: field(b, 5),
            rdev: (field(b, 9), field(b, 10)),
            data: b[data_at..data_at + size].to_vec(),
        });
        b = &b[end..];
    }
    out
}

fn without_lost_found(d: &Path) {
    fs::remove_dir(d.join("lost+found")).unwrap();
}

#[test]
fn check_pin_names_both_hashes_on_a_mismatch() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("f");
    fs::write(&f, "x").unwrap();
    let actual = sha256_file(&f).unwrap();
    let pin = Pin {
        name: "demo pin 1",
        url: "https://example.invalid/demo",
        sha256: "00ff",
    };
    let e = check_pin(&f, &pin).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::InvalidData);
    let msg = e.to_string();
    for want in ["demo pin 1", "00ff", actual.as_str()] {
        assert!(msg.contains(want), "{msg} lacks {want}");
    }
    let ok = Pin {
        sha256: Box::leak(actual.into_boxed_str()),
        ..pin
    };
    check_pin(&f, &ok).unwrap();
}

#[test]
fn initramfs_is_newc_with_console_and_init() {
    let agent = b"agent bytes!!";
    let es = parse_newc(&initramfs(agent));
    let names: Vec<_> = es.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["dev", "dev/console", "init", "TRAILER!!!"]);
    assert_eq!(es[0].mode, 0o040755);
    assert_eq!((es[0].nlink, es[1].nlink, es[2].nlink), (2, 1, 1));
    assert_eq!(es[1].mode, 0o020600);
    assert_eq!(es[1].rdev, (5, 1));
    assert_eq!(es[2].mode, 0o100755);
    assert_eq!(es[2].data, agent);
    for e in &es {
        assert_eq!((e.uid, e.gid, e.mtime), (0, 0, 0));
    }
}

#[test]
fn initramfs_is_deterministic() {
    assert_eq!(initramfs(b"a"), initramfs(b"a"));
    assert_ne!(initramfs(b"a"), initramfs(b"b"));
}

#[test]
fn size_mib_leaves_room() {
    let d = tempfile::tempdir().unwrap();
    let img = d.path().join("i.ext4");
    let many = d.path().join("many");
    fs::create_dir(&many).unwrap();
    for i in 0..2000 {
        fs::write(many.join(format!("f{i}")), "x").unwrap();
    }
    ext4(Some(&many), &img, size_mib(&many).unwrap()).unwrap();
    let big = d.path().join("big");
    fs::create_dir(&big).unwrap();
    fs::write(big.join("blob"), vec![7u8; 40 << 20]).unwrap();
    ext4(Some(&big), &img, size_mib(&big).unwrap()).unwrap();
    // (4 MiB + 4 KiB for the entry) * 5/4 = 5 MiB + 5 KiB, rounded up to 6 MiB, plus 16 MiB.
    let four = d.path().join("four");
    fs::create_dir(&four).unwrap();
    fs::write(four.join("f"), vec![0u8; 4 << 20]).unwrap();
    assert_eq!(size_mib(&four).unwrap(), 22);
}

#[test]
fn ext4_round_trip_keeps_the_tree() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    fs::create_dir_all(src.join("sub/deeper")).unwrap();
    fs::write(src.join("a.txt"), "hi\n").unwrap();
    fs::write(src.join("sub/deeper/b"), vec![1u8; 100_000]).unwrap();
    symlink("a.txt", src.join("l")).unwrap();
    let img = d.path().join("i.ext4");
    ext4(Some(&src), &img, size_mib(&src).unwrap()).unwrap();
    let out = d.path().join("out");
    fs::create_dir(&out).unwrap();
    extract(&img, &out).unwrap();
    without_lost_found(&out);
    assert_eq!(tree_hash(&out).unwrap(), tree_hash(&src).unwrap());
}

#[test]
fn ext4_round_trip_keeps_odd_names() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    fs::create_dir(&src).unwrap();
    for name in [&b"a b"[..], b"new\nline", b"x\xff"] {
        fs::write(src.join(OsStr::from_bytes(name)), name).unwrap();
    }
    let img = d.path().join("i.ext4");
    ext4(Some(&src), &img, size_mib(&src).unwrap()).unwrap();
    let out = d.path().join("out");
    fs::create_dir(&out).unwrap();
    extract(&img, &out).unwrap();
    without_lost_found(&out);
    assert_eq!(fs::read_dir(&out).unwrap().count(), 3);
    assert_eq!(tree_hash(&out).unwrap(), tree_hash(&src).unwrap());
}

#[test]
fn empty_ext4_extracts_to_nothing_but_lost_found() {
    let d = tempfile::tempdir().unwrap();
    let img = d.path().join("i.ext4");
    ext4(None, &img, 16).unwrap();
    let features = std::process::Command::new("debugfs")
        .args(["-R", "features"])
        .arg(&img)
        .output()
        .unwrap();
    let features = String::from_utf8_lossy(&features.stdout);
    assert!(features.contains("extent"), "{features}");
    assert!(!features.contains("has_journal"), "{features}");
    let out = d.path().join("out");
    fs::create_dir(&out).unwrap();
    extract(&img, &out).unwrap();
    let names: Vec<_> = fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["lost+found"]);
}

#[test]
fn extract_refuses_a_dest_with_whitespace_or_quotes() {
    let d = tempfile::tempdir().unwrap();
    let img = d.path().join("i.ext4");
    ext4(None, &img, 16).unwrap();
    for bad in ["a b", "a\"b", "a'b", "a\tb", "a\nb"] {
        let dest = d.path().join(bad);
        fs::create_dir(&dest).unwrap();
        let e = extract(&img, &dest).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::InvalidInput, "{bad:?}");
        assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);
    }
}
