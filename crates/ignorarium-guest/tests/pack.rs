mod common;
use common::{entry, file, link, pack, packed};
use ignorarium_guest::pack::{ENTRY_MAX, Link, out_caps, unpack, write};
use ignorarium_guest::tree::{hash_rows, link_row, row, tree_hash, tree_rows};
use rustix::fs::{Mode, OFlags};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs;
use std::io::{ErrorKind, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, PermissionsExt, symlink};
use std::path::Path;
use std::process::Command;

/// Packs and unpacks `src`. Both sides hash `src`, and what unpack made, with the links it
/// recorded, is `src`. Returns the folder unpack made.
fn round_trip(src: &Path) -> tempfile::TempDir {
    let out = tempfile::tempdir().unwrap();
    let u = unpack(&mut packed(src).as_slice(), out.path(), 1 << 30).unwrap();
    assert_eq!(u.hash, tree_hash(src).unwrap());
    let mut rows = tree_rows(out.path()).unwrap();
    rows.extend(u.links.iter().map(|l| link_row(&l.path, &l.target)));
    rows.sort();
    assert_eq!(rows, tree_rows(src).unwrap());
    out
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
    let out = round_trip(p);
    assert!(out.path().join("empty").is_dir());
    // An "other" entry comes back as a FIFO that only its owner may open.
    let fifo = fs::symlink_metadata(out.path().join("f")).unwrap();
    assert!(fifo.file_type().is_fifo());
    assert_eq!(fifo.permissions().mode() & 0o7777, 0o600);
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
    assert_eq!(write(d.path(), &mut v, u64::MAX).unwrap().bytes, 9);
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

/// Each link's path and target, in pack order: names in byte order, a folder before what it
/// holds.
const LINKS: [(&[u8], &[u8]); 6] = [
    (b"deep", b"a/../../.."),
    (b"gone", b"nowhere"),
    (b"latest", b"/tmp/x"),
    (b"raw", b"\xff/x"),
    (b"sub/back", b"../.."),
    (b"up", b"../x"),
];

#[test]
fn any_link_target_is_carried_as_data() {
    let src = tempfile::tempdir().unwrap();
    fs::write(src.path().join("a.txt"), "hi\n").unwrap();
    fs::create_dir(src.path().join("sub")).unwrap();
    let at = |root: &Path, path: &[u8]| root.join(OsStr::from_bytes(path));
    for (path, target) in LINKS {
        symlink(OsStr::from_bytes(target), at(src.path(), path)).unwrap();
    }
    let mut v = Vec::new();
    let p = write(src.path(), &mut v, u64::MAX).unwrap();
    let out = tempfile::tempdir().unwrap();
    let u = unpack(&mut v.as_slice(), out.path(), 1 << 20).unwrap();
    let want: Vec<Link> = LINKS
        .iter()
        .map(|(path, target)| Link {
            path: path.to_vec(),
            target: target.to_vec(),
        })
        .collect();
    assert_eq!(u.links, want);
    for (path, _) in LINKS {
        let made = fs::symlink_metadata(at(out.path(), path));
        assert_eq!(made.unwrap_err().kind(), ErrorKind::NotFound, "{path:?}");
    }
    assert_eq!(fs::read(out.path().join("a.txt")).unwrap(), b"hi\n");
    assert!(out.path().join("sub").is_dir());
    let hash = tree_hash(src.path()).unwrap();
    assert_eq!((&p.hash, &u.hash), (&hash, &hash));
    // A target that is not UTF-8 is hashed as its raw bytes.
    let rows = tree_rows(src.path()).unwrap();
    let raw = rows.iter().find(|r| r.path == "raw").unwrap();
    assert_eq!(raw.digest, hex::encode(Sha256::digest(b"\xff/x")));
}

#[test]
fn empty_nul_and_overlong_targets_are_still_refused() {
    let long = [b't'; 4096];
    let cases: [(&[u8], &str); 3] = [
        (b"", "empty link target"),
        (b"a\0b", "link target holds a NUL byte"),
        (&long, "link target is too long"),
    ];
    for (target, why) in cases {
        assert_eq!(
            refusal(&pack(&[link(b"l", target)])),
            format!("entry \"l\": {why}")
        );
    }
    // 4095 bytes and its NUL fill `PATH_MAX`: the longest target Linux makes.
    let longest = pack(&[link(b"l", &[b't'; 4095])]);
    let out = tempfile::tempdir().unwrap();
    let u = unpack(&mut longest.as_slice(), out.path(), 1 << 20).unwrap();
    assert_eq!(u.links[0].target, [b't'; 4095]);
}

#[test]
fn a_child_under_a_link_is_still_refused() {
    let under = pack(&[link(b"a", b"/x"), file(b"a/b", b"x")]);
    assert_eq!(
        refusal(&under),
        "entry \"a/b\": parent is not an earlier directory"
    );
    // Nothing is made for the link, so only the path list stops a second entry at its path.
    let again = pack(&[link(b"a", b"x"), file(b"a", b"y")]);
    assert_eq!(refusal(&again), "entry \"a\": duplicate path");
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

/// `n` folders of `len`-byte parts, one in the other, and a file `x` as the last part: a
/// file path of `n * len + n - 1` bytes. Returns the pack and the file's path.
fn deep_pack(n: usize, len: usize) -> (Vec<u8>, Vec<u8>) {
    let mut parts = Vec::new();
    let mut path = Vec::new();
    for i in 0..n {
        if i > 0 {
            path.push(b'/');
        }
        path.extend(std::iter::repeat_n(b'p', len));
        parts.push(if i < n - 1 {
            entry(b'd', &path)
        } else {
            file(&path, b"x")
        });
    }
    (pack(&parts), path)
}

/// The bytes of the file at `rel` below `dir`, opened relative to `dir`, so the whole path
/// may be past `PATH_MAX`.
fn read_below(dir: &Path, rel: &[u8]) -> Vec<u8> {
    let at = rustix::fs::open(dir, OFlags::DIRECTORY | OFlags::RDONLY, Mode::empty()).unwrap();
    let f = rustix::fs::openat(&at, OsStr::from_bytes(rel), OFlags::RDONLY, Mode::empty());
    let mut bytes = Vec::new();
    fs::File::from(f.unwrap()).read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn a_path_of_4095_bytes_unpacks_below_any_dest_and_4096_is_refused() {
    // The host makes each entry relative to `dest`, so the length of `dest` does not count.
    let top = tempfile::tempdir().unwrap();
    let dest = top.path().join("d".repeat(250));
    fs::create_dir(&dest).unwrap();
    let (longest, path) = deep_pack(16, 255);
    assert_eq!(path.len(), 4095);
    let u = unpack(&mut longest.as_slice(), &dest, 1 << 20).unwrap();
    let digest = hex::encode(Sha256::digest(b"x"));
    assert_eq!(u.hash, hash_rows(&[row(&path, "file", digest)]).unwrap());
    assert_eq!(read_below(&dest, &path), b"x");
    // 4096 bytes and the NUL do not fit in `PATH_MAX`.
    let (over, path) = deep_pack(17, 240);
    assert_eq!(path.len(), 4096);
    let msg = refusal(&over);
    assert!(msg.ends_with(": path is too long"), "{msg}");
}

#[test]
fn a_tree_past_path_max_is_refused_as_what_the_payload_left() {
    // A payload can make it with calls relative to a folder; the walk cannot name its end.
    let d = tempfile::tempdir().unwrap();
    let part = "d".repeat(200);
    let mut at = rustix::fs::open(d.path(), OFlags::DIRECTORY | OFlags::RDONLY, Mode::empty());
    for _ in 0..21 {
        let fd = at.unwrap();
        rustix::fs::mkdirat(&fd, part.as_str(), Mode::from_raw_mode(0o755)).unwrap();
        at = rustix::fs::openat(&fd, part.as_str(), OFlags::DIRECTORY, Mode::empty());
    }
    let e = write(d.path(), &mut Vec::new(), 1 << 30).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::InvalidData, "{e}");
    let msg = e.to_string();
    assert!(msg.starts_with("an output path is too long: "), "{msg}");
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
            "link under link",
            pack(&[link(b"a", b"sub"), link(b"a/b", b"/etc")]),
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

#[test]
fn a_full_out_always_fits_the_pack() {
    // An entry's block, a link entry's kind and two lengths, a 4096-byte path and a
    // 4095-byte target: the most one entry adds besides file data.
    assert_eq!(ENTRY_MAX, 4096 + 1 + 4 + 4096 + 4 + 4095);
    assert_eq!(out_caps(1 << 20), (1 << 19, (1 << 19) / ENTRY_MAX));
    for limit in [1 << 20, (1 << 20) + 1, 16 << 20, 256 << 20] {
        let (bytes, inodes) = out_caps(limit);
        assert!(inodes >= 2, "{limit}: {inodes} inodes");
        // tmpfs rounds its size up to a page, and its root takes one inode. The pack
        // adds its magic and end byte.
        let worst = 9 + bytes.next_multiple_of(4096) + (inodes - 1) * ENTRY_MAX;
        assert!(
            worst <= limit,
            "{limit}: a full /out packs to {worst} bytes"
        );
    }
    let d = tempfile::tempdir().unwrap();
    let (bytes, _) = out_caps(1 << 20);
    fs::write(d.path().join("full"), vec![1u8; bytes as usize]).unwrap();
    write(d.path(), &mut Vec::new(), 1 << 20).unwrap();
}
