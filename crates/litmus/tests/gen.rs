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

fn spec_text(d: &Path, name: &str) -> String {
    fs::read_to_string(d.join(name)).unwrap()
}

const SPECS: [&str; 2] = ["spec/pilot.toml", "spec/litmus.toml"];

#[test]
fn bad_seed_refused() {
    for seed in [
        "", "A", "a_b", "a b", "a/b", "-a", "a-", "a--b", "a\"b", "a\nb",
    ] {
        let d = tempfile::tempdir().unwrap();
        assert!(generate(&shape(seed), d.path()).is_err(), "{seed:?}");
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 0, "{seed:?}");
    }
    let d = tempfile::tempdir().unwrap();
    generate(&shape("a-1"), d.path()).unwrap();
}

#[test]
fn expected_verdicts() {
    let d = tempfile::tempdir().unwrap();
    generate(&shape("a"), d.path()).unwrap();
    assert_eq!(
        spec_text(d.path(), "expected.json"),
        r#"{"good":"pass","bad":"fail","edge":"fail"}"#
    );
}

#[test]
fn payload_is_executable() {
    let d = tempfile::tempdir().unwrap();
    generate(&shape("a"), d.path()).unwrap();
    let meta = fs::metadata(d.path().join("payload/run")).unwrap();
    assert!(meta.len() > 0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(meta.permissions().mode() & 0o111, 0);
    }
}

#[test]
fn environment_reaches_both_specs() {
    for env in ["", &"ab".repeat(32)] {
        let d = tempfile::tempdir().unwrap();
        let s = Shape {
            environment: env.into(),
            ..shape("a")
        };
        generate(&s, d.path()).unwrap();
        for spec in SPECS {
            let line = format!("environment = \"{env}\"\n");
            assert!(spec_text(d.path(), spec).contains(&line), "{spec} {env}");
        }
    }
}

#[test]
fn params_only_when_non_zero() {
    let d = tempfile::tempdir().unwrap();
    generate(&shape("a"), d.path()).unwrap();
    assert!(!spec_text(d.path(), "spec/litmus.toml").contains("params"));

    let s = Shape {
        work_ms: 7,
        mem_mib: 9,
        ..shape("a")
    };
    generate(&s, d.path()).unwrap();
    for spec in SPECS {
        let text = spec_text(d.path(), spec);
        assert_eq!(
            text.matches("params = { work_ms = 7, mem_mib = 9 }")
                .count(),
            4
        );
        assert_eq!(text.matches("[[design.conditions]]").count(), 4);
    }
}

#[test]
fn deviation_only_without_environment() {
    let marker = "[[pins.deviations]]";
    let d = tempfile::tempdir().unwrap();
    generate(&shape("a"), d.path()).unwrap();
    assert!(spec_text(d.path(), "spec/litmus.toml").contains(marker));
    assert!(!spec_text(d.path(), "spec/pilot.toml").contains(marker));

    let s = Shape {
        environment: "ab".repeat(32),
        ..shape("a")
    };
    generate(&s, d.path()).unwrap();
    assert!(!spec_text(d.path(), "spec/litmus.toml").contains(marker));
}

#[test]
fn environment_flag_reaches_spec() {
    let d = tempfile::tempdir().unwrap();
    let env = "cd".repeat(32);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_litmus"))
        .args(["gen", "--seed", "a", "--environment", &env, "--out"])
        .arg(d.path().join("study"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    for spec in SPECS {
        let text = spec_text(&d.path().join("study"), spec);
        assert!(text.contains(&format!("environment = \"{env}\"")));
    }
}
