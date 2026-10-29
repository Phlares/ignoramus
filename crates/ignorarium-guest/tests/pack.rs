mod common;
use common::{entry, file, link, pack, packed};
use ignorarium_guest::pack::{unpack, write};
use ignorarium_guest::tree::tree_hash;
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::process::Command;

fn round_trip(src: &std::path::Path) {
    let out = tempfile::tempdir().unwrap();
    unpack(&mut packed(src).as_slice(), out.path(), 1 << 30).unwrap();
    assert_eq!(tree_hash(out.path()).unwrap(), tree_hash(src).unwrap());
}

#[test]
fn pack_round_trip_keeps_the_tree() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    fs::write(p.join("a.txt"), "hi\n").unwrap();
    fs::write(p.join("big"), vec![9u8; 300_000]).unwrap();
    fs::create_dir_all(p.join("sub/deeper")).unwrap();
    fs::write(p.join("sub/deeper/b"), "yo\n").unwrap();
    fs::create_dir(p.join("empty")).unwrap();
    symlink("a.txt", p.join("l")).unwrap();
    symlink("/etc/passwd", p.join("sub/abs")).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(p.join("f"))
            .status()
            .unwrap()
            .success()
    );
    round_trip(p);
    let out = tempfile::tempdir().unwrap();
    unpack(&mut packed(p).as_slice(), out.path(), 1 << 30).unwrap();
    assert!(out.path().join("empty").is_dir());
}

#[test]
fn pack_round_trip_keeps_odd_names() {
    let d = tempfile::tempdir().unwrap();
    for name in [&b"a b"[..], b"new\nline", b"x\xff"] {
        fs::write(d.path().join(OsStr::from_bytes(name)), name).unwrap();
    }
    round_trip(d.path());
}

#[test]
fn the_pack_lists_a_dir_before_its_children() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir(d.path().join("sub")).unwrap();
    fs::write(d.path().join("sub/b"), "x").unwrap();
    let mut want = b"IGNPACK1d".to_vec();
    want.extend_from_slice(&3u32.to_be_bytes());
    want.extend_from_slice(b"subf");
    want.extend_from_slice(&5u32.to_be_bytes());
    want.extend_from_slice(b"sub/b");
    want.extend_from_slice(&1u64.to_be_bytes());
    want.extend_from_slice(b"xe");
    assert_eq!(packed(d.path()), want);
}

#[test]
fn the_pack_lists_names_in_byte_order() {
    let d = tempfile::tempdir().unwrap();
    for name in ["b", "a", "B", "a-c"] {
        fs::write(d.path().join(name), "").unwrap();
    }
    let want = pack(&["B", "a", "a-c", "b"].map(|n| file(n.as_bytes(), b"")));
    assert_eq!(packed(d.path()), want);
}

#[test]
fn an_empty_dir_packs_to_magic_and_end() {
    let d = tempfile::tempdir().unwrap();
    let mut v = Vec::new();
    assert_eq!(write(d.path(), &mut v).unwrap(), 9);
    assert_eq!(v, b"IGNPACK1e");
}

#[test]
fn bytes_after_the_end_are_ignored() {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("a"), "x").unwrap();
    let mut v = packed(d.path());
    v.extend_from_slice(&[0u8; 4096]);
    let out = tempfile::tempdir().unwrap();
    unpack(&mut v.as_slice(), out.path(), 1 << 20).unwrap();
    assert_eq!(tree_hash(out.path()).unwrap(), tree_hash(d.path()).unwrap());
}

/// Sixteen 255-byte parts: every prefix is a legal path, but the last is too long once
/// joined to any `dest`.
fn deep_pack() -> Vec<u8> {
    let mut parts = Vec::new();
    let mut path = Vec::new();
    for i in 0..16 {
        if i > 0 {
            path.push(b'/');
        }
        path.extend_from_slice(&[b'p'; 255]);
        parts.push(if i < 15 {
            entry(b'd', &path)
        } else {
            file(&path, b"x")
        });
    }
    pack(&parts)
}

#[test]
fn hostile_packs_are_refused() {
    let long_name = vec![b'a'; 4097];
    let mut past_end = entry(b'f', b"a");
    past_end.extend_from_slice(&100u64.to_be_bytes());
    past_end.extend_from_slice(b"short");
    let mut huge = entry(b'f', b"a");
    huge.extend_from_slice(&u64::MAX.to_be_bytes());
    let cases: Vec<(&str, Vec<u8>, u64)> = vec![
        ("dotdot", pack(&[file(b"../x", b"x")]), 1 << 20),
        ("absolute", pack(&[file(b"/etc/x", b"x")]), 1 << 20),
        (
            "dot",
            pack(&[entry(b'd', b"a"), file(b"a/./b", b"x")]),
            1 << 20,
        ),
        (
            "empty part",
            pack(&[entry(b'd', b"a"), file(b"a//b", b"x")]),
            1 << 20,
        ),
        ("empty", pack(&[file(b"", b"x")]), 1 << 20),
        (
            "trailing slash",
            pack(&[entry(b'd', b"a"), file(b"a/", b"x")]),
            1 << 20,
        ),
        ("top dot", pack(&[entry(b'd', b".")]), 1 << 20),
        ("top dotdot", pack(&[entry(b'd', b"..")]), 1 << 20),
        (
            "nested dotdot",
            pack(&[entry(b'd', b"a"), entry(b'd', b"a/..")]),
            1 << 20,
        ),
        ("lone slash", pack(&[entry(b'd', b"/")]), 1 << 20),
        ("long part", pack(&[file(&[b'p'; 256], b"x")]), 1 << 20),
        ("empty link target", pack(&[link(b"l", b"")]), 1 << 20),
        ("nul", pack(&[file(b"a\0b", b"x")]), 1 << 20),
        ("long", pack(&[file(&long_name, b"x")]), 1 << 20),
        ("dup", pack(&[file(b"a", b"x"), file(b"a", b"y")]), 1 << 20),
        (
            "dup kinds",
            pack(&[entry(b'd', b"a"), file(b"a", b"y")]),
            1 << 20,
        ),
        (
            "under link",
            pack(&[link(b"l", b".."), file(b"l/x", b"x")]),
            1 << 20,
        ),
        (
            "under file",
            pack(&[file(b"f", b""), file(b"f/x", b"x")]),
            1 << 20,
        ),
        ("no parent", pack(&[file(b"a/b", b"x")]), 1 << 20),
        (
            "parent later",
            pack(&[file(b"a/b", b"x"), entry(b'd', b"a")]),
            1 << 20,
        ),
        ("past stream end", pack(&[past_end]), 1 << 20),
        ("huge length", pack(&[huge]), 1 << 20),
        ("over limit", pack(&[file(b"a", &[1u8; 100])]), 60),
        (
            "sum over limit",
            pack(&[file(b"a", &[1u8; 30]), file(b"b", &[1u8; 30])]),
            90,
        ),
        ("all zero", vec![0u8; 4096], 1 << 20),
        ("short magic", b"IGNPAC".to_vec(), 1 << 20),
        ("wrong magic", b"IGNPACK2e".to_vec(), 1 << 20),
        ("bad kind", pack(&[entry(b'z', b"a")]), 1 << 20),
        ("truncated path", b"IGNPACK1f\0\0\0\x09ab".to_vec(), 1 << 20),
        ("no end", b"IGNPACK1".to_vec(), 1 << 20),
        ("link target nul", pack(&[link(b"l", b"a\0b")]), 1 << 20),
        (
            "link target at the path limit",
            pack(&[link(b"l", &[b't'; 4096])]),
            1 << 20,
        ),
        ("path past the limit with dest", deep_pack(), 1 << 20),
    ];
    for (name, bytes, limit) in cases {
        let outer = tempfile::tempdir().unwrap();
        let dest = outer.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let e = unpack(&mut bytes.as_slice(), &dest, limit).expect_err(name);
        assert_eq!(e.kind(), ErrorKind::InvalidData, "{name}: {e}");
        let siblings: Vec<_> = fs::read_dir(outer.path()).unwrap().collect();
        assert_eq!(siblings.len(), 1, "{name}: something outside dest");
    }
}

#[test]
fn a_refusal_names_the_entry() {
    let outer = tempfile::tempdir().unwrap();
    let bytes = pack(&[file(b"ok", b"x"), file(b"../evil", b"x")]);
    let e = unpack(&mut bytes.as_slice(), outer.path(), 1 << 20).unwrap_err();
    assert!(e.to_string().contains("../evil"), "{e}");
}

#[test]
fn unpack_needs_an_empty_dest() {
    let d = tempfile::tempdir().unwrap();
    let bytes = pack(&[]);
    fs::write(d.path().join("x"), "").unwrap();
    assert!(unpack(&mut bytes.as_slice(), d.path(), 1 << 20).is_err());
    assert!(unpack(&mut bytes.as_slice(), &d.path().join("missing"), 1 << 20).is_err());
}

#[test]
fn an_absolute_path_into_the_outer_dir_is_refused() {
    let outer = tempfile::tempdir().unwrap();
    let dest = outer.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let target = outer.path().join("x");
    let bytes = pack(&[file(target.as_os_str().as_bytes(), b"x")]);
    let e = unpack(&mut bytes.as_slice(), &dest, 1 << 20).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::InvalidData);
    assert!(!target.exists());
}

#[test]
fn each_entry_costs_a_block_of_the_limit() {
    let names: Vec<Vec<u8>> = (0..100).map(|i| format!("{i:04}").into_bytes()).collect();
    let bytes = pack(&names.iter().map(|n| entry(b'd', n)).collect::<Vec<_>>());
    let exact = 100 * 4096 + bytes.len() as u64;
    let out = tempfile::tempdir().unwrap();
    unpack(&mut bytes.as_slice(), out.path(), exact).unwrap();
    assert_eq!(fs::read_dir(out.path()).unwrap().count(), 100);
    let out = tempfile::tempdir().unwrap();
    let e = unpack(&mut bytes.as_slice(), out.path(), exact - 1).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::InvalidData);
}
