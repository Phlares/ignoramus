use bundle::manifest::{self, Entry};
use bundle::{Error, tree};
use std::fs;
use std::path::Path;

const PIN: &str = "f73b792c4fa995abb4cbc6a9c60f3082a574a40f673ead338f0d5ab2bd64cbb8";
const RENDERED: &str = "98ea6e4f216f2fb4b69fff9b3a44842c38686ca685f3f55dc48c5d3fb1107be4  a.txt\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  d/b.txt\n";

fn study(dir: &Path) {
    fs::write(dir.join("a.txt"), "hi\n").unwrap();
    fs::create_dir(dir.join("d")).unwrap();
    fs::write(dir.join("d/b.txt"), "").unwrap();
}

fn paths(files: &[tree::Found]) -> Vec<&str> {
    files.iter().map(|f| f.path.as_str()).collect()
}

fn refused(e: Error) -> Vec<bundle::Problem> {
    match e {
        Error::Refused { problems, .. } => problems,
        other => panic!("expected Refused, got {other:?}"),
    }
}

#[test]
fn render_matches_sha256sum_format() {
    let t = tempfile::tempdir().unwrap();
    study(t.path());
    let entries = manifest::scan(t.path()).unwrap();
    assert_eq!(
        entries.iter().map(|e| e.size).collect::<Vec<_>>(),
        vec![3, 0]
    );
    assert_eq!(
        entries[0],
        Entry {
            path: "a.txt".into(),
            sha256: "98ea6e4f216f2fb4b69fff9b3a44842c38686ca685f3f55dc48c5d3fb1107be4".into(),
            size: 3
        }
    );
    let text = manifest::render(&entries);
    assert_eq!(text, RENDERED);
    assert_eq!(manifest::sha256_bytes(text.as_bytes()), PIN);
}

#[test]
fn walk_sorts_by_bytes() {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("b"), "").unwrap();
    fs::create_dir(t.path().join("a")).unwrap();
    fs::write(t.path().join("a/z"), "").unwrap();
    fs::write(t.path().join("B"), "").unwrap();
    fs::write(t.path().join("a.b"), "").unwrap();
    let (files, problems) = tree::walk(t.path()).unwrap();
    assert_eq!(paths(&files), ["B", "a.b", "a/z", "b"]);
    assert!(problems.is_empty());
}

#[test]
fn walk_refuses_unsafe_names() {
    for name in ["x\ny", "x\ry", "x\\y", "x\u{1}y", "x\u{7f}y"] {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("ok.txt"), "").unwrap();
        if fs::write(t.path().join(name), "").is_err() {
            continue;
        }
        let (files, problems) = tree::walk(t.path()).unwrap();
        assert_eq!(paths(&files), ["ok.txt"], "{name:?}");
        assert_eq!(problems.len(), 1, "{name:?}");
        assert_eq!(problems[0].code, "E-PUB-001");
        assert!(problems[0].path.starts_with("x"), "{name:?}");
        assert!(problems[0].path.ends_with("y"), "{name:?}");
        assert_eq!(
            problems[0].instruction,
            "Replace links and special files with regular files, and rename files to UTF-8 names without control characters or backslashes."
        );
    }
}

#[cfg(unix)]
#[test]
fn walk_refuses_non_utf8_names() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("ok.txt"), "").unwrap();
    fs::write(t.path().join(OsStr::from_bytes(b"bad\xff")), "").unwrap();
    let (files, problems) = tree::walk(t.path()).unwrap();
    assert_eq!(paths(&files), ["ok.txt"]);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].code, "E-PUB-001");
    assert_eq!(problems[0].path, "bad\u{fffd}");
}

#[cfg(unix)]
#[test]
fn walk_refuses_links_without_reading_them() {
    use std::os::unix::fs::symlink;
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("key"), "secret").unwrap();
    fs::create_dir(outside.path().join("sub")).unwrap();
    fs::write(outside.path().join("sub/leak.txt"), "secret").unwrap();
    let t = tempfile::tempdir().unwrap();
    fs::create_dir(t.path().join("data")).unwrap();
    fs::write(t.path().join("data/ok.txt"), "").unwrap();
    symlink(outside.path().join("key"), t.path().join("data/key")).unwrap();
    symlink(outside.path().join("sub"), t.path().join("dirlink")).unwrap();
    let (files, problems) = tree::walk(t.path()).unwrap();
    assert_eq!(paths(&files), ["data/ok.txt"]);
    let paths: Vec<_> = problems.iter().map(|p| p.path.as_str()).collect();
    assert_eq!(paths, ["data/key", "dirlink"]);
    assert!(problems.iter().all(|p| p.code == "E-PUB-001"));
    let err = manifest::scan(t.path()).unwrap_err();
    assert_eq!(refused(err).len(), 2);
}

#[test]
fn check_accepts_an_untouched_folder_and_returns_the_pin() {
    let t = tempfile::tempdir().unwrap();
    study(t.path());
    let text = manifest::render(&manifest::scan(t.path()).unwrap());
    fs::write(t.path().join(manifest::NAME), text).unwrap();
    assert_eq!(manifest::check(t.path()).unwrap(), PIN);
}

#[test]
fn check_refuses_edits_additions_and_removals() {
    let cases: [(&str, &str, &str); 3] = [
        ("a.txt", "edit", "hash differs from the manifest"),
        ("c.txt", "add", "not in the manifest"),
        ("d/b.txt", "remove", "listed in the manifest but missing"),
    ];
    for (path, op, message) in cases {
        let t = tempfile::tempdir().unwrap();
        study(t.path());
        let text = manifest::render(&manifest::scan(t.path()).unwrap());
        fs::write(t.path().join(manifest::NAME), text).unwrap();
        match op {
            "edit" => fs::write(t.path().join("a.txt"), "ho\n").unwrap(),
            "add" => fs::write(t.path().join("c.txt"), "x").unwrap(),
            _ => fs::remove_file(t.path().join("d/b.txt")).unwrap(),
        }
        let problems = refused(manifest::check(t.path()).unwrap_err());
        assert_eq!(problems.len(), 1, "{op}");
        assert_eq!(problems[0].code, "E-PUB-015");
        assert_eq!(problems[0].path, path);
        assert_eq!(problems[0].message, message);
        assert_eq!(
            problems[0].instruction,
            "Rebuild the bundle; never edit a built bundle."
        );
    }
}

#[test]
fn refused_names_are_shown_escaped() {
    let cases = [
        ("a\u{1b}[31mb", "a\\u{1b}[31mb"),
        ("a\u{202e}b", "a\\u{202e}b"),
        ("a\u{85}b", "a\\u{85}b"),
    ];
    for (name, shown) in cases {
        let t = tempfile::tempdir().unwrap();
        if fs::write(t.path().join(name), "").is_err() {
            continue;
        }
        let (files, problems) = tree::walk(t.path()).unwrap();
        assert!(paths(&files).is_empty(), "{shown}");
        assert_eq!(problems.len(), 1, "{shown}");
        assert_eq!(problems[0].code, "E-PUB-001");
        assert_eq!(problems[0].path, shown);
    }
}

#[test]
fn shown_escapes_only_hidden_chars_and_backslash() {
    assert_eq!(tree::shown("a b"), "a b");
    assert_eq!(tree::shown("é"), "é");
    assert_eq!(tree::shown("a\\b"), "a\\\\b");
    assert_eq!(
        tree::shown("\u{9f}\u{a0}\u{2069}\u{2066}"),
        "\\u{9f}\u{a0}\\u{2069}\\u{2066}"
    );
    assert_eq!(
        tree::shown("\u{61c}\u{200e}\u{200f}\u{202a}"),
        "\\u{61c}\\u{200e}\\u{200f}\\u{202a}"
    );
}

#[test]
fn manifest_paths_are_shown_escaped() {
    let t = tempfile::tempdir().unwrap();
    let hash = manifest::sha256_bytes(b"");
    fs::write(
        t.path().join(manifest::NAME),
        format!("{hash}  x\u{1b}[31my\n"),
    )
    .unwrap();
    let problems = refused(manifest::check(t.path()).unwrap_err());
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].path, "x\\u{1b}[31my");
    assert_eq!(problems[0].message, "listed in the manifest but missing");
}

#[test]
fn check_reports_every_mismatch_at_once() {
    let t = tempfile::tempdir().unwrap();
    study(t.path());
    let text = manifest::render(&manifest::scan(t.path()).unwrap());
    fs::write(t.path().join(manifest::NAME), text).unwrap();
    fs::write(t.path().join("a.txt"), "ho\n").unwrap();
    fs::write(t.path().join("c.txt"), "x").unwrap();
    fs::remove_file(t.path().join("d/b.txt")).unwrap();
    assert_eq!(refused(manifest::check(t.path()).unwrap_err()).len(), 3);
}

#[cfg(unix)]
#[test]
fn open_refuses_a_file_swapped_for_a_link() {
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("key"), "secret").unwrap();
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("a.txt"), "secret").unwrap();
    let (files, _) = tree::walk(t.path()).unwrap();
    fs::remove_file(t.path().join("a.txt")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("key"), t.path().join("a.txt")).unwrap();
    let e = tree::open(t.path(), &files[0]).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(e.to_string(), "a.txt changed while the bundle was read");
}

#[cfg(unix)]
#[test]
fn open_refuses_a_parent_swapped_for_a_link() {
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("a.txt"), "secret").unwrap();
    let t = tempfile::tempdir().unwrap();
    fs::create_dir(t.path().join("d")).unwrap();
    fs::write(t.path().join("d/a.txt"), "secret").unwrap();
    let (files, _) = tree::walk(t.path()).unwrap();
    fs::rename(t.path().join("d"), t.path().join("d.away")).unwrap();
    std::os::unix::fs::symlink(outside.path(), t.path().join("d")).unwrap();
    let e = tree::open(t.path(), &files[0]).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn open_refuses_a_file_resized_after_the_walk() {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("a.txt"), "hi").unwrap();
    let (files, _) = tree::walk(t.path()).unwrap();
    fs::write(t.path().join("a.txt"), "hi there").unwrap();
    let e = tree::open(t.path(), &files[0]).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
}
