//! The environment root: a saved image extracted once from the store, and how a run picks it.
mod common;
use common::{Lab, edited, lab, named, skip_level1};
use ignorarium::backend::level0::Level0;
use ignorarium::backend::level1::Level1;
use ignorarium::backend::rootfs::{RootfsError, from_store};
use ignorarium::backend::{Backend, Probe, RowJob, RowOutcome, Watch};
use ignorarium::environ::Builder;
use ignorarium::runner::{self, RunError, RunStatus, environment, prepare_with};
use ignorarium::store::Store;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

const BOX: &str = env!("CARGO_BIN_EXE_ignorarium");
const ABSENT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn tar(args: &[&str], dir: &Path) {
    let out = Command::new("tar")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Put a file or a link (`name -> target` when `target` is set) under `root`.
fn put(root: &Path, name: &str, text: &str, target: Option<&str>) {
    let p = root.join(name);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    match target {
        Some(t) => std::os::unix::fs::symlink(t, &p).unwrap(),
        None => fs::write(&p, text).unwrap(),
    }
}

/// A docker-archive in the store: one layer per folder in `layers`, in order, and a
/// `manifest.json` that lists `names` (the layer paths) unless `manifest` says otherwise.
fn archive(store: &Store, layers: &[&Path], manifest: Option<&str>) -> String {
    archive_as(store, layers, manifest, &[])
}

/// `archive` with extra `tar` options for each layer.
fn archive_as(store: &Store, layers: &[&Path], manifest: Option<&str>, extra: &[&str]) -> String {
    let d = TempDir::new().unwrap();
    let mut names = Vec::new();
    for (i, l) in layers.iter().enumerate() {
        let name = format!("l{i}/layer.tar");
        fs::create_dir(d.path().join(format!("l{i}"))).unwrap();
        let out = d.path().join(&name);
        let head = ["-cf", out.to_str().unwrap(), "-C", l.to_str().unwrap()];
        tar(&[&head[..], extra, &["."]].concat(), d.path());
        names.push(format!("\"{name}\""));
    }
    let text = manifest.map(String::from).unwrap_or_else(|| {
        format!(
            "[{{\"Config\":\"c.json\",\"RepoTags\":[],\"Layers\":[{}]}}]",
            names.join(",")
        )
    });
    fs::write(d.path().join("manifest.json"), text).unwrap();
    let image = d.path().join("image.tar");
    let mut members = vec!["manifest.json".to_string()];
    members.extend((0..layers.len()).map(|i| format!("l{i}")));
    let members: Vec<&str> = members.iter().map(String::as_str).collect();
    tar(
        &[&["-cf", image.to_str().unwrap()][..], &members].concat(),
        d.path(),
    );
    store.put(&fs::read(image).unwrap()).unwrap()
}

fn store() -> (TempDir, Store) {
    let d = TempDir::new().unwrap();
    let s = Store::open(d.path()).unwrap();
    (d, s)
}

fn files_under(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(base: &Path, at: &Path, out: &mut Vec<String>) {
        for e in fs::read_dir(at).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
            out.push(rel);
            if fs::symlink_metadata(&p).unwrap().is_dir() {
                walk(base, &p, out);
            }
        }
    }
    walk(root, root, &mut out);
    out.sort();
    out
}

#[test]
fn from_store_applies_layers_and_whiteouts() {
    let (_h, store) = store();
    let (a, b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    put(a.path(), "usr/a", "A", None);
    put(a.path(), "usr/keep", "K", None);
    put(a.path(), "etc/x/1", "1", None);
    put(a.path(), "bin", "", Some("usr/bin"));
    put(b.path(), "usr/.wh.a", "", None);
    put(b.path(), "etc/x/.wh..wh..opq", "", None);
    put(b.path(), "etc/x/2", "2", None);
    let digest = archive(&store, &[a.path(), b.path()], None);
    let root = from_store(&store, &digest).unwrap();
    assert_eq!(root, store.root.join("rootfs").join(&digest));
    assert_eq!(
        files_under(&root),
        ["bin", "etc", "etc/x", "etc/x/2", "usr", "usr/keep"]
    );
    assert_eq!(
        fs::read_link(root.join("bin")).unwrap(),
        Path::new("usr/bin")
    );
    assert_eq!(fs::read_to_string(root.join("etc/x/2")).unwrap(), "2");
}

#[test]
fn from_store_keeps_only_the_bound_folders() {
    let (_h, store) = store();
    let a = TempDir::new().unwrap();
    put(a.path(), "var/log/y", "y", None);
    put(a.path(), "dev/null", "", None);
    put(a.path(), "home/u/z", "z", None);
    put(a.path(), "lib64/ld", "ld", None);
    put(a.path(), "lib/l", "l", None);
    put(a.path(), "usr/bin/tool", "t", None);
    put(a.path(), "etcetera/q", "q", None);
    let fifo = a.path().join("etc/fifo");
    fs::create_dir_all(fifo.parent().unwrap()).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let digest = archive(&store, &[a.path()], None);
    let root = from_store(&store, &digest).unwrap();
    assert_eq!(
        files_under(&root),
        [
            "etc",
            "lib",
            "lib/l",
            "lib64",
            "lib64/ld",
            "usr",
            "usr/bin",
            "usr/bin/tool"
        ]
    );
}

#[test]
fn from_store_runs_once_per_digest() {
    let (_h, store) = store();
    let a = TempDir::new().unwrap();
    put(a.path(), "usr/a", "A", None);
    let digest = archive(&store, &[a.path()], None);
    let first = from_store(&store, &digest).unwrap();
    let before = fs::metadata(first.join("usr/a"))
        .unwrap()
        .modified()
        .unwrap();
    // A file the image does not hold survives only if the second call leaves the tree alone.
    fs::write(first.join("usr/mark"), "m").unwrap();
    let second = from_store(&store, &digest).unwrap();
    assert_eq!(first, second);
    assert_eq!(fs::read_to_string(second.join("usr/mark")).unwrap(), "m");
    assert_eq!(
        fs::metadata(second.join("usr/a"))
            .unwrap()
            .modified()
            .unwrap(),
        before
    );
}

#[test]
fn from_store_leaves_no_temp_folder_beside_the_result() {
    let (_h, store) = store();
    let a = TempDir::new().unwrap();
    put(a.path(), "usr/a", "A", None);
    let digest = archive(&store, &[a.path()], None);
    from_store(&store, &digest).unwrap();
    let names: Vec<String> = fs::read_dir(store.root.join("rootfs"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [digest]);
}

#[test]
fn from_store_refuses_an_image_that_is_not_in_the_store() {
    let (_h, store) = store();
    assert!(matches!(from_store(&store, ABSENT), Err(RootfsError::Missing(d)) if d == ABSENT));
    assert!(matches!(
        from_store(&store, "../x"),
        Err(RootfsError::Store(_))
    ));
}

#[test]
fn from_store_refuses_a_bad_archive_and_publishes_nothing() {
    let (_h, store) = store();
    let junk = store.put(b"not a tar file").unwrap();
    assert!(matches!(
        from_store(&store, &junk),
        Err(RootfsError::Tar(_))
    ));
    let a = TempDir::new().unwrap();
    put(a.path(), "usr/a", "A", None);
    for manifest in ["[]", "{", "[{\"Layers\":[]}]"] {
        let digest = archive(&store, &[a.path()], Some(manifest));
        let err = from_store(&store, &digest).unwrap_err();
        assert!(matches!(err, RootfsError::Format(_)), "{manifest}: {err}");
        assert!(!store.root.join("rootfs").join(&digest).exists());
    }
    let names: Vec<_> = fs::read_dir(store.root.join("rootfs")).unwrap().collect();
    assert!(names.is_empty());
}

#[test]
fn from_store_refuses_a_layer_path_that_leaves_the_archive() {
    let (_h, store) = store();
    let a = TempDir::new().unwrap();
    put(a.path(), "usr/a", "A", None);
    for path in ["../l0/layer.tar", "/l0/layer.tar", "l0/../l0/layer.tar", ""] {
        let m = format!("[{{\"Layers\":[\"{path}\"]}}]");
        let digest = archive(&store, &[a.path()], Some(&m));
        let err = from_store(&store, &digest).unwrap_err();
        assert!(matches!(err, RootfsError::Unsafe(_)), "{path}: {err}");
    }
}

#[test]
fn from_store_refuses_to_write_through_a_link() {
    let (_h, store) = store();
    let outside = TempDir::new().unwrap();
    let (a, b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    put(
        a.path(),
        "etc/l",
        "",
        Some(outside.path().to_str().unwrap()),
    );
    put(b.path(), "etc/l/pwn", "x", None);
    let digest = archive(&store, &[a.path(), b.path()], None);
    let err = from_store(&store, &digest).unwrap_err();
    assert!(matches!(err, RootfsError::Unsafe(_)), "{err}");
    assert!(!outside.path().join("pwn").exists());
    // The same layer holding the link and the write: `etc/mm` is renamed to `etc/m` in the tar.
    let c = TempDir::new().unwrap();
    put(
        c.path(),
        "etc/m",
        "",
        Some(outside.path().to_str().unwrap()),
    );
    put(c.path(), "etc/mm/pwn", "x", None);
    let digest = archive_as(
        &store,
        &[c.path()],
        None,
        &["--transform=s,^./etc/mm,./etc/m,"],
    );
    let err = from_store(&store, &digest).unwrap_err();
    assert!(matches!(err, RootfsError::Unsafe(_)), "{err}");
    assert!(!outside.path().join("pwn").exists());
}

#[test]
fn a_whiteout_never_follows_a_link() {
    let (_h, store) = store();
    let (a, b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    put(a.path(), "usr/d/f", "keep me", None);
    put(a.path(), "etc/l", "", Some("../usr/d"));
    put(b.path(), "etc/l/.wh.f", "", None);
    let digest = archive(&store, &[a.path(), b.path()], None);
    let err = from_store(&store, &digest).unwrap_err();
    assert!(matches!(err, RootfsError::Unsafe(_)), "{err}");
}

#[test]
fn a_whiteout_removes_a_link_without_touching_its_target() {
    let (_h, store) = store();
    let (a, b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    put(a.path(), "usr/d/f", "keep", None);
    put(a.path(), "etc/l", "", Some("../usr/d"));
    put(b.path(), "etc/.wh.l", "", None);
    let digest = archive(&store, &[a.path(), b.path()], None);
    let root = from_store(&store, &digest).unwrap();
    assert!(fs::symlink_metadata(root.join("etc/l")).is_err());
    assert_eq!(fs::read_to_string(root.join("usr/d/f")).unwrap(), "keep");
}

#[test]
fn a_read_only_folder_in_a_layer_does_not_block_the_next_layer() {
    let (_h, store) = store();
    let (a, b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    put(a.path(), "usr/ro/f", "f", None);
    put(a.path(), "usr/ro/g", "g", None);
    fs::set_permissions(
        a.path().join("usr/ro"),
        std::os::unix::fs::PermissionsExt::from_mode(0o555),
    )
    .unwrap();
    put(b.path(), "usr/ro/.wh.f", "", None);
    put(b.path(), "usr/ro/h", "h", None);
    let digest = archive(&store, &[a.path(), b.path()], None);
    let root = from_store(&store, &digest).unwrap();
    assert_eq!(files_under(&root.join("usr/ro")), ["g", "h"]);
    // Restore the bit so the temp folder can be cleaned.
    fs::set_permissions(
        a.path().join("usr/ro"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
}

// -- how a run picks the environment ------------------------------------------------------

/// Stands in for a sandbox backend: only its level matters to `prepare_with`.
struct Stub(u8);

impl Backend for Stub {
    fn level(&self) -> u8 {
        self.0
    }
    fn probe(&self) -> Probe {
        Probe::Available
    }
    fn launcher(&self) -> String {
        "stub".into()
    }
    fn run_row(&self, _: &RowJob, _: &dyn Watch) -> io::Result<RowOutcome> {
        Err(io::Error::other("stub"))
    }
}

/// A one-layer image holding `usr/a` in the store.
fn small_image(store: &Store) -> String {
    let a = TempDir::new().unwrap();
    put(a.path(), "usr/a", "A", None);
    archive(store, &[a.path()], None)
}

fn pinned(lab: &Lab, id: &str, digest: &str) -> PathBuf {
    let d = digest.to_string();
    edited(
        lab,
        id,
        move |t| t.replace("environment = \"\"", &format!("environment = \"{d}\"")),
        common::PAYLOAD,
    )
}

fn store_of(lab: &Lab) -> Store {
    Store::open(&lab.home.root).unwrap()
}

#[test]
fn a_pinned_environment_must_be_in_the_store() {
    let lab = lab();
    pinned(&lab, "pin", ABSENT);
    let err = prepare_with(&lab.home, "pin", vec![Box::new(Stub(1))])
        .err()
        .unwrap();
    assert!(
        matches!(&err, RunError::EnvironmentMissing(d) if d == ABSENT),
        "{err}"
    );
    assert_eq!(
        err.to_string(),
        format!("the pinned environment {ABSENT} is not in the store")
    );
}

#[test]
fn a_pinned_environment_in_the_store_becomes_the_root() {
    let lab = lab();
    let digest = small_image(&store_of(&lab));
    pinned(&lab, "pin", &digest);
    let plan = prepare_with(&lab.home, "pin", vec![Box::new(Stub(1))]).unwrap();
    let root = store_of(&lab).root.join("rootfs").join(&digest);
    assert_eq!(plan.rootfs.as_deref(), Some(root.as_path()));
    assert_eq!(plan.environment, digest);
    assert!(root.join("usr/a").is_file());
    // The `sha256:` form names the same image.
    pinned(&lab, "pin2", &format!("sha256:{digest}"));
    let plan = prepare_with(&lab.home, "pin2", vec![Box::new(Stub(1))]).unwrap();
    assert_eq!(plan.environment, digest);
}

#[test]
fn level0_ignores_the_environment() {
    let lab = lab();
    let dir = pinned(&lab, "zero", ABSENT);
    // A recipe folder too: a build here would fail for want of a builder or a base.
    fs::create_dir(dir.join("env")).unwrap();
    let plan = prepare_with(&lab.home, "zero", vec![Box::new(Level0)]).unwrap();
    assert_eq!(plan.backend.level(), 0);
    assert_eq!(plan.rootfs, None);
    assert_eq!(plan.environment, "host");
    assert!(!lab.home.root.join("store/rootfs").exists());
}

#[test]
fn no_pin_and_no_recipe_runs_on_the_host_root() {
    let lab = lab();
    common::plain(&lab, "host");
    let plan = prepare_with(&lab.home, "host", vec![Box::new(Stub(1))]).unwrap();
    assert_eq!(plan.rootfs, None);
    assert_eq!(plan.environment, "host");
}

#[test]
fn a_recipe_without_a_builder_is_refused() {
    let lab = lab();
    let dir = common::plain(&lab, "nob");
    let env = dir.join("env");
    fs::create_dir(&env).unwrap();
    let store = store_of(&lab);
    let err = environment(&store, &dir, "", None).err().unwrap();
    assert!(matches!(&err, RunError::Environment(_)), "{err}");
    assert_eq!(
        err.to_string(),
        "environment: the study has an env/ recipe but neither docker nor podman is on PATH; \
         build the image elsewhere and pin its store digest in pins.environment"
    );
    // A pin wins over the recipe: nothing is built.
    let digest = small_image(&store);
    let got = environment(&store, &dir, &digest, None).unwrap();
    assert_eq!(got.name, digest);
    assert!(got.built.is_none());
    assert_eq!(got.rootfs, Some(store.root.join("rootfs").join(&digest)));
}

#[test]
fn a_builder_that_fails_is_a_run_error() {
    let lab = lab();
    let dir = common::plain(&lab, "badb");
    fs::create_dir(dir.join("env")).unwrap();
    fs::write(dir.join("env/Dockerfile"), "FROM scratch\n").unwrap();
    fs::write(dir.join("env/uv.lock"), "").unwrap();
    let b = Builder::with_program("/nonexistent/builder");
    let err = environment(&store_of(&lab), &dir, "", Some(&b))
        .err()
        .unwrap();
    assert!(
        matches!(&err, RunError::Environment(m) if m.contains("neither docker nor podman")),
        "{err}"
    );
}

#[test]
fn a_recipe_folder_missing_a_file_is_a_run_error() {
    let lab = lab();
    let dir = common::plain(&lab, "half");
    fs::create_dir(dir.join("env")).unwrap();
    fs::write(dir.join("env/Dockerfile"), "FROM scratch\n").unwrap();
    let b = Builder::with_program("/nonexistent/builder");
    let err = environment(&store_of(&lab), &dir, "", Some(&b))
        .err()
        .unwrap();
    assert!(
        matches!(&err, RunError::Environment(m) if m.contains("uv.lock")),
        "{err}"
    );
}

#[test]
fn the_start_event_and_row_records_name_the_environment() {
    let lab = lab();
    let digest = small_image(&store_of(&lab));
    let dir = pinned(&lab, "rec", &digest);
    let plan = prepare_with(&lab.home, "rec", vec![Box::new(Level0)]).unwrap();
    assert_eq!(runner::run(&plan).unwrap(), RunStatus::Done);
    let events = common::events_of(&dir);
    assert_eq!(
        named(&events, "run.start")[0]["attrs"]["environment"],
        "host"
    );
    for r in common::rows_of(&lab, "rec") {
        assert_eq!(r.body["environment"], "host");
    }
}

// -- level 1, for real --------------------------------------------------------------------

/// A one-layer image with a shell, the libraries it loads and `etc/os-release`.
fn shell_image(store: &Store) -> String {
    let a = TempDir::new().unwrap();
    let ldd = Command::new("ldd").arg("/bin/sh").output().unwrap();
    let mut files = vec!["/bin/sh".to_string()];
    files.extend(
        String::from_utf8_lossy(&ldd.stdout)
            .split_whitespace()
            .filter(|t| t.starts_with('/'))
            .map(String::from),
    );
    for f in files {
        let at = if f.starts_with("/usr/") {
            f.clone()
        } else {
            format!("/usr{f}")
        };
        let dest = a.path().join(at.trim_start_matches('/'));
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::copy(&f, &dest).unwrap();
    }
    for link in ["bin", "lib", "lib64"] {
        put(a.path(), link, "", Some(&format!("usr/{link}")));
    }
    put(a.path(), "etc/os-release", "NAME=fixture-os\n", None);
    archive(store, &[a.path()], None)
}

#[test]
fn a_row_runs_in_the_pinned_image() {
    let lab = lab();
    let level1 = Level1::with(Level1::new().bwrap, BOX.into());
    if skip_level1() {
        return;
    }
    assert_eq!(level1.probe(), Probe::Available);
    let digest = shell_image(&store_of(&lab));
    let payload =
        "#!/bin/sh\nread x < /etc/os-release\nprintf '%s' \"$x\" > \"$IGNORARIUM_OUT/os\"\n";
    let dir = edited(
        &lab,
        "e2e",
        |t| t.replace("environment = \"\"", &format!("environment = \"{digest}\"")),
        payload,
    );
    let plan = prepare_with(&lab.home, "e2e", vec![Box::new(level1)]).unwrap();
    assert_eq!(plan.environment, digest);
    assert_eq!(runner::run(&plan).unwrap(), RunStatus::Done);
    let rows = common::rows_of(&lab, "e2e");
    assert_eq!(rows.len(), 6);
    for r in &rows {
        assert_eq!(r.body["exit"], "ok");
        assert_eq!(r.body["environment"], digest.as_str());
        let out = dir.join(format!("run/rows/{}/out/os", r.row.unwrap()));
        assert_eq!(fs::read_to_string(out).unwrap(), "NAME=fixture-os");
    }
}

#[test]
#[ignore = "needs docker or podman"]
fn a_built_environment_runs_at_level1() {
    let lab = lab();
    let level1 = Level1::with(Level1::new().bwrap, BOX.into());
    if skip_level1() {
        return;
    }
    let payload = "#!/bin/sh\ncat /etc/os-release > \"$IGNORARIUM_OUT/os\"\n";
    let dir = edited(&lab, "built", |t| t, payload);
    let env = dir.join("env");
    fs::create_dir(&env).unwrap();
    // Set BUSYBOX_DIGEST to a real digest of a public busybox image.
    let base = std::env::var("BUSYBOX_DIGEST").expect("BUSYBOX_DIGEST names the base image");
    fs::write(
        env.join("Dockerfile"),
        format!("FROM busybox@sha256:{base}\n"),
    )
    .unwrap();
    fs::write(env.join("uv.lock"), "").unwrap();
    let plan = prepare_with(&lab.home, "built", vec![Box::new(level1)]).unwrap();
    assert_eq!(runner::run(&plan).unwrap(), RunStatus::Done);
    let out = fs::read_to_string(dir.join("run/rows/0/out/os")).unwrap();
    assert!(out.contains("Buildroot"), "{out}");
    let events = common::events_of(&dir);
    assert_eq!(named(&events, "env.build").len(), 1);
}
