use ignorarium::spec::model::{Role, Spec};
use litmus::{Shape, generate};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Every file under `dir` as relative path to bytes.
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
                out.insert(parts.join("/"), fs::read(&p).unwrap());
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

/// Generates `shape` in a new folder.
fn generated(shape: &Shape) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    generate(shape, PAYLOAD.as_ref(), d.path()).unwrap();
    d
}

/// The spec `name` under `d` as the JSON the box hashes.
fn raw(d: &Path, name: &str) -> serde_json::Value {
    ignorarium::spec::load(&d.join(name)).unwrap().value
}

/// The spec `name` under `d`, as the box parses it.
fn spec(d: &Path, name: &str) -> Spec {
    serde_json::from_value(raw(d, name)).unwrap()
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

const SPECS: [&str; 2] = ["spec/pilot.toml", "spec/litmus.toml"];
const PAYLOAD: &str = env!("CARGO_BIN_EXE_litmus");

#[test]
fn defaults() {
    let d = Shape::default();
    assert_eq!((d.pilot, d.first, d.reserve, d.units), (8, 12, 60, 2));
}

#[test]
fn same_seed_same_bytes() {
    let (a, b) = (generated(&shape("a")), generated(&shape("a")));
    let t = tree(a.path());
    assert!(t.contains_key("spec/litmus.toml"));
    assert_eq!(t, tree(b.path()));
}

#[test]
fn two_seeds_share_no_hash() {
    let (a, b) = (generated(&shape("a")), generated(&shape("b")));
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
    let d = generated(&s);
    for name in SPECS {
        let inputs = spec(d.path(), name).inputs;
        assert!(!inputs.is_empty());
        for i in inputs {
            let file = fs::read(d.path().join(&i.path)).unwrap();
            assert_eq!(i.sha256, digest(&file), "{}", i.path);
            assert_eq!(i.units, Some(s.units));
        }
    }
}

#[test]
fn roles_and_counts() {
    let s = shape("a");
    let d = generated(&s);
    let (pilot, litmus) = (
        spec(d.path(), "spec/pilot.toml").inputs,
        spec(d.path(), "spec/litmus.toml").inputs,
    );
    let count = |l: &[ignorarium::spec::model::Input], role| {
        l.iter().filter(|i| i.role == role).count() as u32
    };
    assert_eq!(pilot.len() as u32, s.pilot);
    assert_eq!(count(&pilot, Role::Confirmation), s.pilot);
    assert_eq!(count(&litmus, Role::Confirmation), s.first);
    assert_eq!(count(&litmus, Role::Reserve), s.reserve);
    assert_eq!(litmus.len() as u32, s.first + s.reserve);
    assert!(
        pilot
            .iter()
            .all(|p| litmus.iter().all(|l| l.path != p.path))
    );
}

#[test]
fn zero_clusters_refused() {
    let d = tempfile::tempdir().unwrap();
    let s = Shape {
        first: 0,
        ..Shape::default()
    };
    assert!(generate(&s, PAYLOAD.as_ref(), d.path()).is_err());
}

#[test]
fn bad_seed_refused() {
    for seed in [
        "", "A", "a_b", "a b", "a/b", "-a", "a-", "a--b", "a\"b", "a\nb",
    ] {
        let d = tempfile::tempdir().unwrap();
        assert!(
            generate(&shape(seed), PAYLOAD.as_ref(), d.path()).is_err(),
            "{seed:?}"
        );
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 0, "{seed:?}");
    }
    generated(&shape("a-1"));
}

#[test]
fn occupied_out_refused() {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("stale.txt"), "x").unwrap();
    assert!(generate(&shape("a"), PAYLOAD.as_ref(), d.path()).is_err());
    assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1);
}

#[test]
fn expected_verdicts() {
    let d = generated(&shape("a"));
    assert_eq!(
        fs::read_to_string(d.path().join("expected.json")).unwrap(),
        r#"{"good":"pass","bad":"fail","edge":"fail"}"#
    );
}

#[test]
fn payload_is_executable() {
    let d = generated(&shape("a"));
    let copy = d.path().join("payload/run");
    assert_eq!(fs::read(&copy).unwrap(), fs::read(PAYLOAD).unwrap());
    let meta = fs::metadata(copy).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(meta.permissions().mode() & 0o111, 0);
    }
}

#[test]
fn environment_reaches_both_specs() {
    for env in ["", &"ab".repeat(32)] {
        let d = generated(&Shape {
            environment: env.into(),
            ..shape("a")
        });
        for name in SPECS {
            assert_eq!(spec(d.path(), name).pins.environment, env, "{name}");
        }
    }
}

#[test]
fn params_only_when_non_zero() {
    let at = |work_ms, mem_mib| -> Vec<Option<serde_json::Value>> {
        let d = generated(&Shape {
            work_ms,
            mem_mib,
            ..shape("a")
        });
        let mut all = Vec::new();
        for name in SPECS {
            let raw = raw(d.path(), name);
            let conditions = raw["design"]["conditions"].as_array().unwrap();
            assert_eq!(conditions.len(), 4, "{name}");
            all.extend(conditions.iter().map(|c| c.get("params").cloned()));
        }
        all
    };
    let both = serde_json::json!({ "work_ms": 7, "mem_mib": 9 });
    assert!(at(0, 0).iter().all(Option::is_none));
    assert!(at(7, 9).iter().all(|p| p.as_ref() == Some(&both)));
}

#[test]
fn deviation_only_without_environment() {
    let deviations = |environment: String, name| {
        let d = generated(&Shape {
            environment,
            ..shape("a")
        });
        spec(d.path(), name).pins.deviations.len()
    };
    assert_eq!(deviations(String::new(), "spec/litmus.toml"), 1);
    assert_eq!(deviations(String::new(), "spec/pilot.toml"), 0);
    assert_eq!(deviations("ab".repeat(32), "spec/litmus.toml"), 0);
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
    for name in SPECS {
        let pinned = spec(&d.path().join("study"), name).pins.environment;
        assert_eq!(pinned, env, "{name}");
    }
}

#[test]
fn bad_environment_refused() {
    for env in [
        "AB".repeat(32),
        "a".repeat(63),
        "a".repeat(65),
        format!("{}\"", "a".repeat(63)),
        format!("{}\n", "a".repeat(63)),
    ] {
        let d = tempfile::tempdir().unwrap();
        let s = Shape {
            environment: env.clone(),
            ..shape("a")
        };
        assert!(generate(&s, PAYLOAD.as_ref(), d.path()).is_err(), "{env:?}");
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 0, "{env:?}");
    }
}
