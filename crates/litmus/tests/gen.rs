use litmus::{Shape, generate};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Every file under `dir` as relative path to bytes, without the payload copy.
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                todo.push(p);
            } else {
                let parts: Vec<_> = p
                    .strip_prefix(dir)
                    .unwrap()
                    .iter()
                    .map(|c| c.to_string_lossy())
                    .collect();
                let rel = parts.join("/");
                if rel != "payload/run" {
                    out.insert(rel, fs::read(&p).unwrap());
                }
            }
        }
    }
    out
}

fn shape(seed: &str) -> Shape {
    Shape {
        seed: seed.into(),
        ..Shape::default()
    }
}

/// The `[[inputs]]` blocks of a spec as (path, sha256, role, units).
fn inputs(spec: &str) -> Vec<(String, String, String, u32)> {
    let mut out: Vec<(String, String, String, u32)> = Vec::new();
    let mut inside = false;
    for line in spec.lines() {
        if line.starts_with('[') {
            inside = line == "[[inputs]]";
            if inside {
                out.push(Default::default());
            }
        } else if let (true, Some((k, v))) = (inside, line.split_once(" = ")) {
            let last = out.last_mut().unwrap();
            let s = v.trim_matches('"').to_string();
            match k {
                "path" => last.0 = s,
                "sha256" => last.1 = s,
                "role" => last.2 = s,
                "units" => last.3 = s.parse().unwrap(),
                _ => {}
            }
        }
    }
    out
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn defaults() {
    let d = Shape::default();
    assert_eq!((d.pilot, d.first, d.reserve, d.units), (8, 12, 60, 2));
}

#[test]
fn same_seed_same_bytes() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    generate(&shape("a"), a.path()).unwrap();
    generate(&shape("a"), b.path()).unwrap();
    let t = tree(a.path());
    assert!(t.contains_key("spec/litmus.toml"));
    assert_eq!(t, tree(b.path()));
}

#[test]
fn two_seeds_share_no_hash() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    generate(&shape("a"), a.path()).unwrap();
    generate(&shape("b"), b.path()).unwrap();
    let hashes = |d: &Path| -> Vec<String> {
        tree(d)
            .iter()
            .filter(|(k, _)| k.starts_with("data/"))
            .map(|(_, v)| digest(v))
            .collect()
    };
    let (ha, hb) = (hashes(a.path()), hashes(b.path()));
    assert!(!ha.is_empty());
    assert!(ha.iter().all(|h| !hb.contains(h)));
}

#[test]
fn pins_match_files() {
    let s = shape("a");
    let d = tempfile::tempdir().unwrap();
    generate(&s, d.path()).unwrap();
    for spec in ["spec/pilot.toml", "spec/litmus.toml"] {
        let list = inputs(&fs::read_to_string(d.path().join(spec)).unwrap());
        assert!(!list.is_empty());
        for (path, sha, _, units) in list {
            assert_eq!(
                sha,
                digest(&fs::read(d.path().join(&path)).unwrap()),
                "{path}"
            );
            assert_eq!(units, s.units);
        }
    }
}

#[test]
fn roles_and_counts() {
    let s = shape("a");
    let d = tempfile::tempdir().unwrap();
    generate(&s, d.path()).unwrap();
    let read = |f: &str| inputs(&fs::read_to_string(d.path().join(f)).unwrap());
    let count = |l: &[(String, String, String, u32)], role: &str| {
        l.iter().filter(|i| i.2 == role).count() as u32
    };
    let (pilot, litmus) = (read("spec/pilot.toml"), read("spec/litmus.toml"));
    assert_eq!(pilot.len() as u32, s.pilot);
    assert_eq!(count(&pilot, "confirmation"), s.pilot);
    assert_eq!(count(&litmus, "confirmation"), s.first);
    assert_eq!(count(&litmus, "reserve"), s.reserve);
    assert_eq!(litmus.len() as u32, s.first + s.reserve);
    assert!(pilot.iter().all(|p| litmus.iter().all(|l| l.0 != p.0)));
}

#[test]
fn zero_clusters_refused() {
    let d = tempfile::tempdir().unwrap();
    let s = Shape {
        first: 0,
        ..Shape::default()
    };
    assert!(generate(&s, d.path()).is_err());
}
