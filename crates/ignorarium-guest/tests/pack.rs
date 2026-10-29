mod common;
use common::{entry, file, link, pack, packed};
use ignorarium_guest::pack::{unpack, write};
use ignorarium_guest::tree::tree_hash;
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::Path;
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
    symlink("deeper/b", p.join("sub/down")).unwrap();
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
    assert_eq!(write(d.path(), &mut v, u64::MAX).unwrap(), 9);
    assert_eq!(v, b"IGNPACK1e");
}

/// Counts every byte offered to it.
struct Offered(u64);
impl std::io::Write for Offered {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `write` and `unpack` both accept `dir` at its cost (stream bytes plus a block per entry)
/// and both refuse it one byte below.
fn same_limit_on_both_sides(dir: &Path, entries: u64) {
    let limit = packed(dir).len() as u64 + entries * 4096;
    let mut v = Vec::new();
    write(dir, &mut v, limit).unwrap();
    unpack(
        &mut v.as_slice(),
        tempfile::tempdir().unwrap().path(),
        limit,
    )
    .unwrap();
    let mut short = Offered(0);
    let e = write(dir, &mut short, limit - 1).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::InvalidData);
    let want = format!("outputs exceed the output limit of {} bytes", limit - 1);
    assert_eq!(e.to_string(), want);
    // Entries are charged a block each, so the bytes offered must fit in what is left.
    let offered = short.0 + entries * 4096;
    assert!(
        offered < limit,
        "offered {offered} bytes against a limit of {}",
        limit - 1
    );
    let out = tempfile::tempdir().unwrap();
    let e = unpack(&mut v.as_slice(), out.path(), limit - 1).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::InvalidData);
}

#[test]
fn write_and_unpack_charge_the_same_limit() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    fs::write(p.join("a"), vec![1u8; 10_000]).unwrap();
    fs::create_dir(p.join("sub")).unwrap();
    fs::write(p.join("sub/b"), "").unwrap();
    symlink("b", p.join("sub/l")).unwrap();
    same_limit_on_both_sides(p, 4);
}

#[test]
fn many_empty_dirs_meet_the_same_limit_on_both_sides() {
    let d = tempfile::tempdir().unwrap();
    for i in 0..300 {
        fs::create_dir(d.path().join(format!("d{i}"))).unwrap();
    }
    same_limit_on_both_sides(d.path(), 300);
}

#[test]
fn link_targets_must_be_relative_without_parent_steps() {
    let refused: [(&[u8], &[u8]); 7] = [
        (b"a", b"/etc"),
        (b"a", b"../x"),
        (b"a", b"../b"),
        (b"d/a", b"../b"),
        (b"d/a", b"../../x"),
        (b"d/a", b"x/../y"),
        // With `l -> .` this chain stays inside by a lexical count, yet climbs out when followed.
        (b"d/m", b"../l/l/../.."),
    ];
    for (path, target) in refused {
        let bytes = pack(&[entry(b'd', b"d"), link(b"l", b"."), link(path, target)]);
        let out = tempfile::tempdir().unwrap();
        let e = unpack(&mut bytes.as_slice(), out.path(), 1 << 20).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::InvalidData, "{e}");
        let named = format!("{:?}", String::from_utf8_lossy(path));
        assert!(e.to_string().contains(&named), "{e}");
        assert!(fs::symlink_metadata(out.path().join(OsStr::from_bytes(path))).is_err());
    }
    let ok = pack(&[
        entry(b'd', b"d"),
        link(b"l", b"."),
        link(b"d/a", b"b"),
        link(b"d/c", b"./b/c"),
    ]);
    let out = tempfile::tempdir().unwrap();
    unpack(&mut ok.as_slice(), out.path(), 1 << 20).unwrap();
    for (path, target) in [("l", "."), ("d/a", "b"), ("d/c", "./b/c")] {
        assert_eq!(
            fs::read_link(out.path().join(path)).unwrap(),
            Path::new(target)
        );
    }
}

#[test]
fn write_refuses_a_link_that_unpack_would_refuse() {
    for target in ["/etc", "../b", "x/../y"] {
        let d = tempfile::tempdir().unwrap();
        symlink(target, d.path().join("a")).unwrap();
        let e = write(d.path(), &mut Vec::new(), u64::MAX).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::InvalidData, "{target}");
        assert!(e.to_string().contains("\"a\""), "{e}");
    }
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
            pack(&[link(b"l", b"."), file(b"l/x", b"x")]),
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

fn refusal(bytes: &[u8]) -> String {
    let out = tempfile::tempdir().unwrap();
    let e = unpack(&mut &bytes[..], out.path(), 1 << 20).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::InvalidData, "{e}");
    e.to_string()
}

#[test]
fn a_path_part_of_255_bytes_is_accepted_and_256_refused() {
    let name = [b'p'; 255];
    let out = tempfile::tempdir().unwrap();
    unpack(
        &mut pack(&[file(&name, b"x")]).as_slice(),
        out.path(),
        1 << 20,
    )
    .unwrap();
    assert!(out.path().join(OsStr::from_bytes(&name)).is_file());
    let msg = refusal(&pack(&[file(&[b'p'; 256], b"x")]));
    assert!(msg.contains("path part is too long"), "{msg}");
}

/// A target of 4096 bytes passes the length cap on names and fails the target check;
/// 4097 bytes fail the cap first. The two refusals say which check fired.
#[test]
fn a_name_may_be_4096_bytes_but_not_4097() {
    let msg = refusal(&pack(&[link(b"l", &[b't'; 4096])]));
    assert!(msg.contains("link target is too long"), "{msg}");
    let msg = refusal(&pack(&[link(b"l", &[b't'; 4097])]));
    assert!(
        msg.contains("link target is longer than 4096 bytes"),
        "{msg}"
    );
}

#[test]
fn a_bad_link_target_field_names_the_entry() {
    let msg = refusal(&pack(&[link(b"l", b"a\0b")]));
    assert!(msg.starts_with("entry \"l\": "), "{msg}");
    assert!(msg.contains("holds a NUL byte"), "{msg}");
    // A pack cut inside the target is named the same way.
    let mut cut = entry(b'l', b"l");
    cut.extend_from_slice(&9u32.to_be_bytes());
    cut.extend_from_slice(b"ab");
    let msg = refusal(&pack(&[cut]));
    assert!(msg.starts_with("entry \"l\": "), "{msg}");
}
