//! The environment root: a saved image extracted once from the store, and how a run picks it.
mod common;
use common::{Lab, edited, lab, named, skip_level1};
use ignorarium::backend::level0::Level0;
use ignorarium::backend::level1::Level1;
use ignorarium::backend::rootfs::{self, Limits, RootfsError, from_store, from_store_capped};
use ignorarium::backend::{Backend, Probe, RowJob, RowOutcome, Watch};
use ignorarium::environ::Builder;
use ignorarium::runner::{self, RunError, RunStatus, environment, prepare_with};
use ignorarium::store::Store;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
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
    let mut tars = Vec::new();
    for (i, l) in layers.iter().enumerate() {
        let out = d.path().join(format!("t{i}.tar"));
        let head = ["-cf", out.to_str().unwrap(), "-C", l.to_str().unwrap()];
        tar(&[&head[..], extra, &["."]].concat(), d.path());
        tars.push(out);
    }
    image(store, &tars, manifest)
}

/// A docker-archive in the store whose layers are the given tar files.
fn image(store: &Store, tars: &[PathBuf], manifest: Option<&str>) -> String {
    let d = TempDir::new().unwrap();
    let mut names = Vec::new();
    for (i, t) in tars.iter().enumerate() {
        let name = format!("l{i}/layer.tar");
        fs::create_dir(d.path().join(format!("l{i}"))).unwrap();
        fs::copy(t, d.path().join(&name)).unwrap();
        names.push(format!("\"{name}\""));
    }
    let text = manifest.map(String::from).unwrap_or_else(|| {
        format!(
            "[{{\"Config\":\"c.json\",\"RepoTags\":[],\"Layers\":[{}]}}]",
            names.join(",")
        )
    });
    fs::write(d.path().join("manifest.json"), text).unwrap();
    let out = d.path().join("image.tar");
    let mut members = vec!["manifest.json".to_string()];
    members.extend((0..tars.len()).map(|i| format!("l{i}")));
    let members: Vec<&str> = members.iter().map(String::as_str).collect();
    tar(
        &[&["-cf", out.to_str().unwrap()][..], &members].concat(),
        d.path(),
    );
    store.put(&fs::read(out).unwrap()).unwrap()
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
    // With the image gone, only the extracted tree can answer.
    let blob = store.path(&digest).unwrap();
    fs::rename(&blob, blob.with_extension("away")).unwrap();
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
fn a_link_or_file_planted_at_the_tree_path_is_never_returned() {
    use std::os::unix::fs::PermissionsExt;
    let (_h, store) = store();
    let a = TempDir::new().unwrap();
    put(a.path(), "usr/a", "A", None);
    let digest = archive(&store, &[a.path()], None);
    let parent = store.root.join("rootfs");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    let host = host_dir();
    let at = parent.join(&digest);
    std::os::unix::fs::symlink(host.path(), &at).unwrap();
    let err = from_store(&store, &digest).unwrap_err();
    assert!(matches!(err, RootfsError::Io(_)), "{err}");
    assert_eq!(fs::read_link(&at).unwrap(), host.path());
    assert_eq!(fs::read_dir(host.path()).unwrap().count(), 1);
    fs::remove_file(&at).unwrap();
    fs::write(&at, "planted").unwrap();
    let err = from_store(&store, &digest).unwrap_err();
    assert!(matches!(err, RootfsError::Io(_)), "{err}");
    assert_eq!(fs::read_to_string(&at).unwrap(), "planted");
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
fn a_manifest_may_name_at_most_256_layers() {
    let (_h, store) = store();
    let a = TempDir::new().unwrap();
    put(a.path(), "usr/a", "A", None);
    // One small layer, named again and again.
    let naming = |n: usize| {
        let names = vec!["\"l0/layer.tar\""; n].join(",");
        archive(
            &store,
            &[a.path()],
            Some(&format!("[{{\"Layers\":[{names}]}}]")),
        )
    };
    let root = from_store(&store, &naming(256)).unwrap();
    assert_eq!(files_under(&root), ["usr", "usr/a"]);
    let over = naming(257);
    let err = from_store(&store, &over).unwrap_err();
    assert!(matches!(err, RootfsError::Format(_)), "{err}");
    assert_eq!(
        err.to_string(),
        "bad image archive: manifest.json lists 257 layers, over the limit of 256"
    );
    assert!(!store.root.join("rootfs").join(&over).exists());
}

#[test]
fn a_link_named_like_the_manifest_or_a_layer_is_not_that_file() {
    let (_h, store) = store();
    let mut b = tar::Builder::new(Vec::new());
    for name in ["manifest.json", "l0/layer.tar"] {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_path(name).unwrap();
        h.set_link_name("elsewhere").unwrap();
        h.set_mode(0o777);
        h.set_size(0);
        h.set_cksum();
        b.append(&h, &b""[..]).unwrap();
    }
    let manifest = br#"[{"Layers":["l0/layer.tar"]}]"#;
    let layer = layer_bytes(&[M::File("usr/a", "A")]);
    for (name, data) in [
        ("manifest.json", &manifest[..]),
        ("l0/layer.tar", &layer[..]),
    ] {
        let mut h = tar::Header::new_gnu();
        h.set_path(name).unwrap();
        h.set_mode(0o644);
        h.set_size(data.len() as u64);
        h.set_cksum();
        b.append(&h, data).unwrap();
    }
    let digest = store.put(&b.into_inner().unwrap()).unwrap();
    let root = from_store(&store, &digest).unwrap();
    assert_eq!(fs::read_to_string(root.join("usr/a")).unwrap(), "A");
}

#[test]
fn the_first_file_of_a_repeated_name_counts() {
    let (_h, store) = store();
    let first = layer_bytes(&[M::File("usr/a", "first")]);
    let second = layer_bytes(&[M::File("usr/a", "second")]);
    let other = layer_bytes(&[M::File("usr/b", "b")]);
    // The second copy of `l0/layer.tar` comes before `l1/layer.tar`, so the pass that finds
    // the layers reads it. The second manifest names a layer the archive lacks.
    let mut b = tar::Builder::new(Vec::new());
    for (name, data) in [
        (
            "manifest.json",
            &br#"[{"Layers":["l0/layer.tar","l1/layer.tar"]}]"#[..],
        ),
        ("manifest.json", &br#"[{"Layers":["l9/layer.tar"]}]"#[..]),
        ("l0/layer.tar", &first[..]),
        ("l0/layer.tar", &second[..]),
        ("l1/layer.tar", &other[..]),
    ] {
        let mut h = tar::Header::new_gnu();
        h.set_path(name).unwrap();
        h.set_mode(0o644);
        h.set_size(data.len() as u64);
        h.set_cksum();
        b.append(&h, data).unwrap();
    }
    let digest = store.put(&b.into_inner().unwrap()).unwrap();
    let root = from_store(&store, &digest).unwrap();
    assert_eq!(files_under(&root), ["usr", "usr/a", "usr/b"]);
    assert_eq!(fs::read_to_string(root.join("usr/a")).unwrap(), "first");
}

#[test]
fn a_layer_the_archive_lacks_is_a_format_error() {
    let (_h, store) = store();
    let a = TempDir::new().unwrap();
    put(a.path(), "usr/a", "A", None);
    let m = r#"[{"Layers":["l0/layer.tar","l9/layer.tar"]}]"#;
    let digest = archive(&store, &[a.path()], Some(m));
    let err = from_store(&store, &digest).unwrap_err();
    assert_eq!(
        err.to_string(),
        "bad image archive: layer l9/layer.tar is missing"
    );
}

#[test]
fn from_store_refuses_a_layer_path_that_leaves_the_archive() {
    let (_h, store) = store();
    let a = TempDir::new().unwrap();
    put(a.path(), "usr/a", "A", None);
    for path in [
        "../l0/layer.tar",
        "/l0/layer.tar",
        "l0/../l0/layer.tar",
        "./l0/layer.tar",
        "",
    ] {
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
    let t = TempDir::new().unwrap();
    let target = outside.path().to_str().unwrap();
    // Layer 1 leaves a link; layer 2 holds only `etc/l/pwn`, with no folder entry for `etc/l`.
    let (a, b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    put(a.path(), "etc/l", "", Some(target));
    put(b.path(), "etc/l/pwn", "x", None);
    let (t1, t2) = (t.path().join("1.tar"), t.path().join("2.tar"));
    layer(a.path(), &t1, &[], &["."]);
    layer(b.path(), &t2, &[], &["etc/l/pwn"]);
    let digest = image(&store, &[t1, t2], None);
    let err = from_store(&store, &digest).unwrap_err();
    assert!(matches!(err, RootfsError::Unsafe(_)), "{err}");
    assert!(!outside.path().join("pwn").exists());
    // The same layer holding the link and the write: `etc/mm` is renamed to `etc/m`.
    let c = TempDir::new().unwrap();
    put(c.path(), "etc/m", "", Some(target));
    put(c.path(), "etc/mm/pwn", "x", None);
    let t3 = t.path().join("3.tar");
    layer(
        c.path(),
        &t3,
        &["--transform=s,^etc/mm,etc/m,"],
        &["etc/m", "etc/mm/pwn"],
    );
    let digest = image(&store, &[t3], None);
    let err = from_store(&store, &digest).unwrap_err();
    assert!(matches!(err, RootfsError::Unsafe(_)), "{err}");
    assert!(!outside.path().join("pwn").exists());
}

#[test]
fn a_folder_entry_replaces_a_link_instead_of_following_it() {
    let (_h, store) = store();
    let outside = TempDir::new().unwrap();
    let (a, b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    put(
        a.path(),
        "etc/l",
        "",
        Some(outside.path().to_str().unwrap()),
    );
    put(b.path(), "etc/l/f", "x", None);
    let digest = archive(&store, &[a.path(), b.path()], None);
    let root = from_store(&store, &digest).unwrap();
    assert!(fs::symlink_metadata(root.join("etc/l")).unwrap().is_dir());
    assert_eq!(fs::read_to_string(root.join("etc/l/f")).unwrap(), "x");
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
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
    fn cpus(&self) -> u32 {
        1
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
    let payload = format!(
        "#!/bin/sh\nread x < /etc/os-release\nprintf '%s' \"$x\" > \"$IGNORARIUM_OUT/os\"\n{}\n",
        common::VALUES
    );
    let dir = edited(
        &lab,
        "e2e",
        |t| t.replace("environment = \"\"", &format!("environment = \"{digest}\"")),
        &payload,
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
    // Only the image holds /bin/busybox; its first passwd line ends in /bin/sh.
    let payload = concat!(
        "#!/bin/sh\n",
        "head -n1 /etc/passwd > \"$IGNORARIUM_OUT/os\"\n",
        "[ -e /bin/busybox ] && echo busybox >> \"$IGNORARIUM_OUT/os\"\n",
        "printf '{\"ppl\": 1.5}' > \"$IGNORARIUM_OUT/values.json\"\n",
    );
    let dir = edited(&lab, "built", |t| t, payload);
    let env = dir.join("env");
    fs::create_dir(&env).unwrap();
    // BUSYBOX_DIGEST is the 64 hex digits of a public busybox image digest.
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
    assert_eq!(out, "root:x:0:0:root:/root:/bin/sh\nbusybox\n");
    let events = common::events_of(&dir);
    assert_eq!(named(&events, "env.build").len(), 1);
}

// -- review fixes ---------------------------------------------------------------------------

/// A layer tar from `members` (paths under `dir`, in this order) with extra `tar` options.
fn layer(dir: &Path, out: &Path, extra: &[&str], members: &[&str]) {
    let head = ["-cf", out.to_str().unwrap(), "-C", dir.to_str().unwrap()];
    tar(&[&head[..], extra, members].concat(), dir);
}

#[test]
fn a_hard_link_cannot_pull_a_host_file_through_a_planted_link() {
    let (_h, store) = store();
    let host = TempDir::new().unwrap();
    fs::write(host.path().join("secret"), "host data").unwrap();
    let (a, b, t) = (
        TempDir::new().unwrap(),
        TempDir::new().unwrap(),
        TempDir::new().unwrap(),
    );
    put(a.path(), "etc/x", "", Some(host.path().to_str().unwrap()));
    // Layer 2: `usr/pull` is a hard link to `etc/x/secret`; the target member is cut out.
    fs::write(b.path().join("real"), "r").unwrap();
    fs::create_dir(b.path().join("usr")).unwrap();
    fs::hard_link(b.path().join("real"), b.path().join("usr/pull")).unwrap();
    let (t1, t2) = (t.path().join("1.tar"), t.path().join("2.tar"));
    layer(a.path(), &t1, &[], &["."]);
    layer(
        b.path(),
        &t2,
        &["--transform=s,^real$,etc/x/secret,"],
        &["real", "usr/pull"],
    );
    tar(
        &["-f", t2.to_str().unwrap(), "--delete", "etc/x/secret"],
        t.path(),
    );
    let digest = image(&store, &[t1, t2], None);
    let err = from_store(&store, &digest).unwrap_err();
    assert!(matches!(err, RootfsError::Unsafe(_)), "{err}");
    assert!(!store.root.join("rootfs").join(&digest).exists());
    assert_eq!(
        fs::read_to_string(host.path().join("secret")).unwrap(),
        "host data"
    );
}

#[test]
fn a_hard_link_to_a_link_or_outside_the_kept_folders_is_refused() {
    let (_h, store) = store();
    let t = TempDir::new().unwrap();
    for (case, target) in [("link", "usr/lnk"), ("outside", "var/x")] {
        let d = TempDir::new().unwrap();
        put(d.path(), "usr/real", "r", None);
        put(d.path(), "usr/lnk", "", Some("real"));
        put(d.path(), "var/x", "x", None);
        fs::hard_link(d.path().join(target), d.path().join("etc-pull")).unwrap();
        let tarf = t.path().join(format!("{case}.tar"));
        layer(
            d.path(),
            &tarf,
            &["--transform=s,^etc-pull$,etc/pull,"],
            &[target, "etc-pull"],
        );
        let digest = image(&store, &[tarf], None);
        let err = from_store(&store, &digest).unwrap_err();
        assert!(matches!(err, RootfsError::Unsafe(_)), "{case}: {err}");
    }
}

#[test]
fn a_busybox_style_hard_link_inside_a_kept_folder_extracts() {
    use std::os::unix::fs::MetadataExt;
    let (_h, store) = store();
    let a = TempDir::new().unwrap();
    put(a.path(), "usr/bin/busybox", "b", None);
    fs::hard_link(
        a.path().join("usr/bin/busybox"),
        a.path().join("usr/bin/sh"),
    )
    .unwrap();
    let digest = archive(&store, &[a.path()], None);
    let root = from_store(&store, &digest).unwrap();
    let x = fs::metadata(root.join("usr/bin/busybox")).unwrap();
    let y = fs::metadata(root.join("usr/bin/sh")).unwrap();
    assert_eq!(x.ino(), y.ino());
    assert_eq!(fs::read_to_string(root.join("usr/bin/sh")).unwrap(), "b");
}

#[test]
fn a_hard_link_reaches_a_file_written_before_other_members() {
    let (_h, store) = store();
    let digest = image_of(
        &store,
        &[layer_bytes(&[
            M::File("usr/a", "A"),
            M::Sym("usr/s", "a"),
            M::File("usr/b", "B"),
            M::Hard("usr/c", "usr/a", "box"),
        ])],
    );
    let root = from_store(&store, &digest).unwrap();
    assert_eq!(fs::read_to_string(root.join("usr/c")).unwrap(), "A");
}

#[test]
fn setuid_and_setgid_bits_do_not_survive_extraction() {
    use std::os::unix::fs::PermissionsExt;
    let (_h, store) = store();
    let a = TempDir::new().unwrap();
    put(a.path(), "usr/bin/su", "s", None);
    put(a.path(), "usr/bin/wall", "w", None);
    let mode = |f: &str, m| {
        let p = a.path().join("usr/bin").join(f);
        fs::set_permissions(p, fs::Permissions::from_mode(m)).unwrap();
    };
    mode("su", 0o4755);
    mode("wall", 0o2755);
    let digest = archive(&store, &[a.path()], None);
    let root = from_store(&store, &digest).unwrap();
    for f in ["su", "wall"] {
        let got = fs::metadata(root.join("usr/bin").join(f))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(got & 0o7000, 0, "{f}: {got:o}");
        assert_eq!(got & 0o777, 0o755, "{f}: {got:o}");
    }
}

#[test]
fn a_root_level_whiteout_removes_a_whole_kept_folder() {
    let (_h, store) = store();
    let (a, b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    put(a.path(), "etc/x/1", "1", None);
    put(a.path(), "usr/y", "y", None);
    put(a.path(), "lib64", "", Some("usr/lib64"));
    put(b.path(), ".wh.etc", "", None);
    put(b.path(), ".wh.lib64", "", None);
    let digest = archive(&store, &[a.path(), b.path()], None);
    let root = from_store(&store, &digest).unwrap();
    assert_eq!(files_under(&root), ["usr", "usr/y"]);
}

#[test]
fn a_whiteout_under_a_missing_folder_makes_no_folder() {
    let (_h, store) = store();
    let digest = image_of(
        &store,
        &[
            layer_bytes(&[M::File("usr/a", "A")]),
            layer_bytes(&[
                M::File("usr/missing/.wh.y", ""),
                M::File("etc/none/.wh..wh..opq", ""),
            ]),
        ],
    );
    let root = from_store(&store, &digest).unwrap();
    assert_eq!(files_under(&root), ["usr", "usr/a"]);
}

#[test]
fn an_opaque_whiteout_on_a_file_leaves_the_file() {
    let (_h, store) = store();
    let digest = image_of(
        &store,
        &[
            layer_bytes(&[M::File("etc/f", "F")]),
            layer_bytes(&[M::File("etc/f/.wh..wh..opq", "")]),
        ],
    );
    let root = from_store(&store, &digest).unwrap();
    assert_eq!(fs::read_to_string(root.join("etc/f")).unwrap(), "F");
}

#[test]
fn a_whiteout_through_a_name_too_long_for_the_disk_fails_the_extraction() {
    let (_h, store) = store();
    // A name over 255 bytes cannot be looked up, so the whiteout cannot be judged: as a
    // folder on the way, or as the name the whiteout removes.
    let long = "n".repeat(300);
    for path in [
        format!("usr/{long}/z/.wh..wh..opq"),
        format!("usr/.wh.{long}"),
    ] {
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_mode(0o644);
        h.set_size(0);
        b.append_data(&mut h, &path, &b""[..]).unwrap();
        let digest = image_of(
            &store,
            &[
                layer_bytes(&[M::File("usr/a", "A")]),
                b.into_inner().unwrap(),
            ],
        );
        let err = from_store(&store, &digest).unwrap_err();
        assert!(
            matches!(&err, RootfsError::Io(e) if e.raw_os_error() == Some(libc::ENAMETOOLONG)),
            "{err}"
        );
    }
}

#[test]
fn a_member_under_a_file_is_a_format_error() {
    let (_h, store) = store();
    let digest = image_of(
        &store,
        &[layer_bytes(&[
            M::File("usr/x", "x"),
            M::File("usr/x/y", "y"),
        ])],
    );
    let err = from_store(&store, &digest).unwrap_err();
    assert!(matches!(err, RootfsError::Format(_)), "{err}");
    assert_eq!(
        err.to_string(),
        "bad image archive: usr/x is a file, not a folder"
    );
}

#[test]
fn a_root_level_whiteout_of_dot_or_dot_dot_removes_nothing() {
    let (_h, store) = store();
    let digest = image_of(
        &store,
        &[
            layer_bytes(&[M::File("usr/a", "A")]),
            layer_bytes(&[
                M::File(".wh...", ""),
                M::File(".wh..", ""),
                M::File("usr/b", "B"),
            ]),
        ],
    );
    let root = from_store(&store, &digest).unwrap();
    assert_eq!(files_under(&root), ["usr", "usr/a", "usr/b"]);
}

#[test]
fn a_whiteout_of_dot_or_dot_dot_below_the_root_removes_nothing() {
    let (_h, store) = store();
    for name in ["usr/.wh..", "usr/.wh..."] {
        let digest = image_of(
            &store,
            &[
                layer_bytes(&[M::File("usr/a", "A"), M::File("etc/e", "E")]),
                layer_bytes(&[M::File(name, ""), M::File("usr/b", "B")]),
            ],
        );
        let root = from_store(&store, &digest).unwrap();
        assert_eq!(
            files_under(&root),
            ["etc", "etc/e", "usr", "usr/a", "usr/b"],
            "{name}"
        );
    }
}

#[test]
fn a_name_with_a_newline_extracts_under_that_name() {
    let (_h, store) = store();
    let a = TempDir::new().unwrap();
    put(a.path(), "etc/ok", "k", None);
    put(a.path(), "etc/a\nb", "x", None);
    put(a.path(), "etc/l", "", Some("t\nu"));
    let digest = archive(&store, &[a.path()], None);
    let root = from_store(&store, &digest).unwrap();
    assert_eq!(fs::read_to_string(root.join("etc/a\nb")).unwrap(), "x");
    assert_eq!(
        fs::read_link(root.join("etc/l")).unwrap(),
        Path::new("t\nu")
    );
    assert_eq!(files_under(&root), ["etc", "etc/a\nb", "etc/l", "etc/ok"]);
}

#[test]
fn a_name_outside_the_kept_folders_may_hold_a_newline() {
    let (_h, store) = store();
    let a = TempDir::new().unwrap();
    put(a.path(), "etc/ok", "k", None);
    put(a.path(), "var/a\nb", "x", None);
    let digest = archive(&store, &[a.path()], None);
    let root = from_store(&store, &digest).unwrap();
    assert_eq!(files_under(&root), ["etc", "etc/ok"]);
}

#[test]
fn a_pin_that_is_not_a_digest_is_not_reported_as_absent() {
    let lab = lab();
    let dir = common::plain(&lab, "badpin");
    let store = store_of(&lab);
    for pin in ["busybox:latest", "ABCDEF", &ABSENT[..63], "sha256:ABC"] {
        let err = environment(&store, &dir, pin, None).err().unwrap();
        assert!(
            matches!(&err, RunError::EnvironmentPin(p) if p == pin),
            "{err}"
        );
        assert_eq!(
            err.to_string(),
            format!("pins.environment {pin:?} is not a store digest")
        );
    }
}

// -- hostile archives, written with the tar crate ------------------------------------------

enum M<'a> {
    File(&'a str, &'a str),
    Sym(&'a str, &'a str),
    /// A hard link: name, target, owner name.
    Hard(&'a str, &'a str, &'a str),
}

/// A layer tar holding `members`. Names and targets are written into the header as given.
fn layer_bytes(members: &[M]) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for m in members {
        let mut h = tar::Header::new_gnu();
        let (name, link, kind, data, owner) = match m {
            M::File(n, d) => (*n, "", tar::EntryType::Regular, d.as_bytes(), "box"),
            M::Sym(n, t) => (*n, *t, tar::EntryType::Symlink, &b""[..], "box"),
            M::Hard(n, t, o) => (*n, *t, tar::EntryType::Link, &b""[..], *o),
        };
        h.set_entry_type(kind);
        h.set_mode(0o644);
        h.set_size(data.len() as u64);
        h.set_username(owner).unwrap();
        let old = h.as_old_mut();
        old.name[..name.len()].copy_from_slice(name.as_bytes());
        old.linkname[..link.len()].copy_from_slice(link.as_bytes());
        h.set_cksum();
        b.append(&h, data).unwrap();
    }
    b.into_inner().unwrap()
}

/// An image in the store whose layers are the given tar bytes.
fn image_of(store: &Store, layers: &[Vec<u8>]) -> String {
    let t = TempDir::new().unwrap();
    let tars: Vec<PathBuf> = layers
        .iter()
        .enumerate()
        .map(|(i, bytes)| {
            let p = t.path().join(format!("{i}.tar"));
            fs::write(&p, bytes).unwrap();
            p
        })
        .collect();
    image(store, &tars, None)
}

fn host_dir() -> TempDir {
    let host = TempDir::new().unwrap();
    fs::write(host.path().join("secret"), "host data").unwrap();
    host
}

/// The image is refused as unsafe, and no host file appeared in the host folder.
fn refused(store: &Store, layers: &[Vec<u8>], host: &TempDir) {
    let digest = image_of(store, layers);
    let err = from_store(store, &digest).unwrap_err();
    assert!(matches!(err, RootfsError::Unsafe(_)), "{err}");
    assert!(!store.root.join("rootfs").join(&digest).exists());
    assert_eq!(fs::read_dir(host.path()).unwrap().count(), 1);
}

#[test]
fn a_hard_link_with_a_crafted_owner_name_is_judged_by_its_real_target() {
    let (_h, store) = store();
    let host = host_dir();
    let h = host.path().to_str().unwrap();
    // The owner text mimics the human listing: "usr/ok link to ..." must not matter.
    let owner = "a b usr/ok link to usr/ok";
    refused(
        &store,
        &[
            layer_bytes(&[M::File("usr/ok", "fine"), M::Sym("etc/x", h)]),
            layer_bytes(&[M::Hard("usr/pull", "etc/x/secret", owner)]),
        ],
        &host,
    );
}

#[test]
fn an_opaque_whiteout_in_a_link_to_a_host_folder_is_refused() {
    let (_h, store) = store();
    let host = host_dir();
    let h = host.path().to_str().unwrap();
    refused(
        &store,
        &[
            layer_bytes(&[M::Sym("etc/x", h)]),
            layer_bytes(&[M::File("etc/x/.wh..wh..opq", "")]),
        ],
        &host,
    );
}

#[test]
fn a_hard_link_through_a_symlink_of_the_same_layer_is_refused() {
    let (_h, store) = store();
    let host = host_dir();
    let h = host.path().to_str().unwrap();
    refused(
        &store,
        &[layer_bytes(&[
            M::Sym("etc/x", h),
            M::Hard("usr/pull", "etc/x/secret", "box"),
        ])],
        &host,
    );
}

#[test]
fn a_hard_link_through_a_lower_layer_symlink_is_refused() {
    let (_h, store) = store();
    let host = host_dir();
    let h = host.path().to_str().unwrap();
    refused(
        &store,
        &[
            layer_bytes(&[M::File("usr/real/f", "x")]),
            layer_bytes(&[
                M::Sym("usr/real", h),
                M::Hard("etc/pull", "usr/real/secret", "box"),
            ]),
        ],
        &host,
    );
    // Even a name that exists as a file below the link is not reachable through it.
    refused(
        &store,
        &[
            layer_bytes(&[M::File("usr/real/f", "x")]),
            layer_bytes(&[
                M::Sym("usr/real", h),
                M::Hard("etc/pull", "usr/real/f", "box"),
            ]),
        ],
        &host,
    );
}

#[test]
fn a_hard_link_to_a_symlink_is_refused() {
    let (_h, store) = store();
    let host = host_dir();
    refused(
        &store,
        &[layer_bytes(&[
            M::File("usr/a", "x"),
            M::Sym("usr/s", "a"),
            M::Hard("etc/p", "usr/s", "box"),
        ])],
        &host,
    );
}

#[test]
fn a_hard_link_with_an_absolute_or_parent_target_is_refused() {
    let (_h, store) = store();
    let host = host_dir();
    for target in [
        "/etc/hostname",
        "../x",
        "usr/../../x",
        "usr/a/../../../x",
        ".",
    ] {
        refused(
            &store,
            &[layer_bytes(&[
                M::File("usr/a", "x"),
                M::Hard("etc/p", target, "box"),
            ])],
            &host,
        );
    }
}

#[test]
fn a_hard_link_to_a_file_not_yet_written_or_outside_the_kept_folders_is_refused() {
    let (_h, store) = store();
    let host = host_dir();
    refused(
        &store,
        &[layer_bytes(&[
            M::Hard("etc/p", "usr/later", "box"),
            M::File("usr/later", "x"),
        ])],
        &host,
    );
    refused(
        &store,
        &[layer_bytes(&[
            M::File("var/x", "x"),
            M::Hard("etc/p", "var/x", "box"),
        ])],
        &host,
    );
}

#[test]
fn a_later_member_replacing_a_hard_link_target_leaves_the_link_on_the_old_file() {
    let (_h, store) = store();
    let host = host_dir();
    let h = host.path().to_str().unwrap();
    let digest = image_of(
        &store,
        &[layer_bytes(&[
            M::File("usr/a", "old"),
            M::Hard("usr/b", "usr/a", "box"),
            M::File("usr/a", "new"),
        ])],
    );
    let root = from_store(&store, &digest).unwrap();
    assert_eq!(fs::read_to_string(root.join("usr/a")).unwrap(), "new");
    assert_eq!(fs::read_to_string(root.join("usr/b")).unwrap(), "old");
    // A target replaced by a link to the host is not a file any more.
    refused(
        &store,
        &[layer_bytes(&[
            M::File("usr/a", "old"),
            M::Sym("usr/a", h),
            M::Hard("usr/c", "usr/a", "box"),
        ])],
        &host,
    );
}

#[test]
fn a_zstd_xz_or_bzip2_layer_is_refused() {
    let (_h, store) = store();
    for (name, magic) in [
        ("zstd", &[0x28, 0xb5, 0x2f, 0xfd][..]),
        ("xz", &[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00][..]),
        ("bzip2", &b"BZh9"[..]),
    ] {
        let mut bytes = magic.to_vec();
        bytes.extend_from_slice(&[0; 600]);
        let digest = image_of(&store, &[bytes]);
        let err = from_store(&store, &digest).unwrap_err();
        assert!(matches!(&err, RootfsError::Format(_)), "{name}: {err}");
        assert_eq!(
            err.to_string(),
            format!("bad image archive: unsupported compression: {name}")
        );
        assert!(!store.root.join("rootfs").join(&digest).exists());
    }
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

/// The files of `root` with their contents (links as `-> target`).
fn tree_of(root: &Path) -> Vec<(String, String)> {
    files_under(root)
        .into_iter()
        .map(|f| {
            let p = root.join(&f);
            let m = fs::symlink_metadata(&p).unwrap();
            let what = if m.is_dir() {
                "dir".to_string()
            } else if m.is_symlink() {
                format!("-> {}", fs::read_link(&p).unwrap().display())
            } else {
                fs::read_to_string(&p).unwrap()
            };
            (f, what)
        })
        .collect()
}

#[test]
fn a_gzip_layer_extracts_the_same_tree_as_its_plain_twin() {
    let (_h, store) = store();
    let layers = [
        layer_bytes(&[
            M::File("usr/bin/busybox", "bb"),
            M::Hard("usr/bin/sh", "usr/bin/busybox", "box"),
            M::Sym("bin", "usr/bin"),
            M::File("etc/os-release", "NAME=x\n"),
        ]),
        layer_bytes(&[M::File(".wh.etc", ""), M::File("lib/l", "l")]),
    ];
    let plain = from_store(&store, &image_of(&store, &layers)).unwrap();
    let zipped: Vec<Vec<u8>> = layers.iter().map(|l| gzip(l)).collect();
    assert_ne!(zipped[0], layers[0]);
    let gz = from_store(&store, &image_of(&store, &zipped)).unwrap();
    assert_ne!(plain, gz);
    assert_eq!(tree_of(&plain), tree_of(&gz));
    assert!(tree_of(&gz).contains(&("usr/bin/sh".into(), "bb".into())));
    assert!(tree_of(&gz).iter().all(|(f, _)| f != "etc"));
    // A plain layer and a gzip layer can sit in one image.
    let mixed = from_store(
        &store,
        &image_of(&store, &[layers[0].clone(), zipped[1].clone()]),
    )
    .unwrap();
    assert_eq!(tree_of(&plain), tree_of(&mixed));
}

#[test]
fn a_hostile_gzip_layer_is_judged_like_a_plain_one() {
    let (_h, store) = store();
    let host = host_dir();
    let h = host.path().to_str().unwrap();
    let layers = [
        gzip(&layer_bytes(&[M::Sym("etc/x", h)])),
        gzip(&layer_bytes(&[M::Hard("usr/pull", "etc/x/secret", "box")])),
    ];
    refused(&store, &layers, &host);
}

/// `from_store_capped` with `limits` fails with `message`, and neither the image's tree nor a
/// temp folder for it is left in the store.
fn fails_cleanly(store: &Store, digest: &str, limits: Limits, message: &str) {
    let err = from_store_capped(store, digest, limits).unwrap_err();
    assert!(matches!(&err, RootfsError::Format(_)), "{err}");
    assert_eq!(err.to_string(), message);
    let left: Vec<String> = fs::read_dir(store.root.join("rootfs"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(digest))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

/// The default limits with an extraction budget of `cap`.
fn budget(cap: u64) -> Limits {
    Limits {
        budget: cap,
        ..rootfs::LIMITS
    }
}

#[test]
fn the_paths_an_extraction_records_are_capped() {
    let (_h, store) = store();
    let digest = image_of(
        &store,
        &[layer_bytes(&[
            M::File("usr/a", "A"),
            M::Sym("usr/s", "a"),
            M::Hard("usr/h", "usr/a", "box"),
        ])],
    );
    // The file, the link and the hard link are recorded: `usr/a`, `usr/s`, `usr/h`.
    let records = |cap| Limits {
        records: cap,
        ..rootfs::LIMITS
    };
    fails_cleanly(
        &store,
        &digest,
        records(14),
        "bad image archive: the paths the extraction records are over 14 bytes",
    );
    let root = from_store_capped(&store, &digest, records(15)).unwrap();
    assert_eq!(fs::read_to_string(root.join("usr/h")).unwrap(), "A");
    assert_eq!(rootfs::LIMITS.records, 256 << 20);
}

/// The image is refused for its extraction budget at `cap`.
fn over_budget(store: &Store, digest: &str, cap: u64) {
    let message = format!(
        "bad image archive: the image is over the extraction budget of {cap} bytes \
         (archive bytes read to find its members and in two passes over each layer, bytes \
         decoded from gzip layers, and 4096 per entry written)"
    );
    fails_cleanly(store, digest, budget(cap), &message);
}

/// What the two passes over a one-layer `image` archive read, the manifest first: its
/// header and its 61 bytes; then its header and block, the `l0/` header and the layer's
/// header.
const OUTER: u64 = 512 + 61 + 4 * 512;

const FENCED: &str = "bad image archive: a header extension is larger than 1 MiB";
const MIB: usize = 1 << 20;

/// Append an extension entry of `kind` holding `data`, then a file `name` holding `text`.
fn extended(b: &mut tar::Builder<Vec<u8>>, kind: tar::EntryType, data: &[u8], name: &str) {
    let mut x = tar::Header::new_gnu();
    x.set_entry_type(kind);
    x.set_path("././@LongLink").unwrap();
    x.set_mode(0o644);
    x.set_size(data.len() as u64);
    x.set_cksum();
    b.append(&x, data).unwrap();
    let mut h = tar::Header::new_gnu();
    h.set_path(name).unwrap();
    h.set_mode(0o644);
    h.set_size(1);
    h.set_cksum();
    b.append(&h, &b"x"[..]).unwrap();
}

/// One PAX record, `<length> <key>=<value>\n`; the length counts its own digits.
fn pax(key: &str, value: &str) -> Vec<u8> {
    let body = format!(" {key}={value}\n");
    let mut len = body.len();
    while len != body.len() + len.to_string().len() {
        len = body.len() + len.to_string().len();
    }
    format!("{len}{body}").into_bytes()
}

/// A path of about 3 KiB under `usr/`, in folders of 200 characters (a name may hold 255).
fn long_path(c: char) -> String {
    let part = c.to_string().repeat(200);
    format!("usr/{}f", format!("{part}/").repeat(15))
}

#[test]
fn long_names_of_a_few_kib_still_extract() {
    let (_h, store) = store();
    let (gnu, pax_path) = (long_path('a'), long_path('b'));
    let mut b = tar::Builder::new(Vec::new());
    extended(&mut b, tar::EntryType::GNULongName, gnu.as_bytes(), "usr/x");
    extended(
        &mut b,
        tar::EntryType::XHeader,
        &pax("path", &pax_path),
        "usr/y",
    );
    let digest = image_of(&store, &[b.into_inner().unwrap()]);
    let root = from_store(&store, &digest).unwrap();
    assert!(gnu.len() > 3000, "{}", gnu.len());
    assert_eq!(fs::read_to_string(root.join(&gnu)).unwrap(), "x");
    assert_eq!(fs::read_to_string(root.join(&pax_path)).unwrap(), "x");
    assert!(!root.join("usr/x").exists() && !root.join("usr/y").exists());
}

#[test]
fn an_oversized_long_name_or_pax_record_is_refused_before_it_is_read() {
    let (_h, store) = store();
    let big = "a".repeat(4 * MIB);
    for (kind, data) in [
        (tar::EntryType::GNULongName, big.clone().into_bytes()),
        (tar::EntryType::XHeader, pax("comment", &big)),
    ] {
        let mut b = tar::Builder::new(Vec::new());
        extended(&mut b, kind, &data, "usr/x");
        let digest = image_of(&store, &[b.into_inner().unwrap()]);
        // Past the archive's two passes, a budget one byte past the fence: the fence error
        // shows the stream gave the crate at most that byte past it, so it never held the
        // extension.
        fails_cleanly(&store, &digest, budget(OUTER + (MIB + 1) as u64), FENCED);
    }
}

#[test]
fn the_fence_is_exact_and_moves_with_each_entry() {
    let (_h, store) = store();
    // Before the first entry: the long-name header, its data, the file header = 1 MiB.
    let layer = |n: usize| {
        let mut b = tar::Builder::new(Vec::new());
        extended(
            &mut b,
            tar::EntryType::GNULongName,
            "v".repeat(n).as_bytes(),
            "var/x",
        );
        b.into_inner().unwrap()
    };
    let at = image_of(
        &store,
        &[layer(MIB - 1024), layer_bytes(&[M::File("usr/a", "A")])],
    );
    assert_eq!(
        files_under(&from_store(&store, &at).unwrap()),
        ["usr", "usr/a"]
    );
    let past = image_of(&store, &[layer(MIB - 1023)]);
    fails_cleanly(&store, &past, rootfs::LIMITS, FENCED);
    // Each entry opens a new window past its own declared data.
    let mut b = tar::Builder::new(Vec::new());
    extended(
        &mut b,
        tar::EntryType::GNULongName,
        &[b'v'; 600 << 10],
        "var/x",
    );
    extended(
        &mut b,
        tar::EntryType::GNULongName,
        &[b'w'; 600 << 10],
        "var/y",
    );
    let mut h = tar::Header::new_gnu();
    h.set_path("var/big").unwrap();
    h.set_mode(0o644);
    h.set_size(4 * MIB as u64);
    h.set_cksum();
    b.append(&h, &vec![0u8; 4 * MIB][..]).unwrap();
    let digest = image_of(
        &store,
        &[
            b.into_inner().unwrap(),
            layer_bytes(&[M::File("usr/a", "A")]),
        ],
    );
    assert_eq!(
        files_under(&from_store(&store, &digest).unwrap()),
        ["usr", "usr/a"]
    );
}

#[test]
fn the_outer_archive_is_fenced_too() {
    let (_h, store) = store();
    let mut b = tar::Builder::new(Vec::new());
    extended(
        &mut b,
        tar::EntryType::GNULongName,
        &vec![b'a'; 4 * MIB],
        "x",
    );
    let manifest = br#"[{"Layers":["l0/layer.tar"]}]"#;
    let layer = layer_bytes(&[M::File("usr/a", "A")]);
    for (name, data) in [
        ("manifest.json", &manifest[..]),
        ("l0/layer.tar", &layer[..]),
    ] {
        let mut h = tar::Header::new_gnu();
        h.set_path(name).unwrap();
        h.set_mode(0o644);
        h.set_size(data.len() as u64);
        h.set_cksum();
        b.append(&h, data).unwrap();
    }
    let digest = store.put(&b.into_inner().unwrap()).unwrap();
    fails_cleanly(&store, &digest, rootfs::LIMITS, FENCED);
}

#[test]
fn the_window_after_an_entry_starts_past_its_padding() {
    let (_h, store) = store();
    // After a 1-byte file and its 511 bytes of padding: the long-name header, its data and
    // the next file header may take 1 MiB.
    let layer = |n: usize| {
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_path("var/a").unwrap();
        h.set_mode(0o644);
        h.set_size(1);
        h.set_cksum();
        b.append(&h, &b"a"[..]).unwrap();
        extended(
            &mut b,
            tar::EntryType::GNULongName,
            "v".repeat(n).as_bytes(),
            "var/x",
        );
        b.into_inner().unwrap()
    };
    let at = image_of(
        &store,
        &[layer(MIB - 1024), layer_bytes(&[M::File("usr/a", "A")])],
    );
    assert_eq!(
        files_under(&from_store(&store, &at).unwrap()),
        ["usr", "usr/a"]
    );
    let past = image_of(&store, &[layer(MIB - 1023)]);
    fails_cleanly(&store, &past, rootfs::LIMITS, FENCED);
}

/// Append a file `name` holding `data` whose header says `header_size` and whose PAX record
/// says `data.len()`.
fn pax_sized(b: &mut tar::Builder<Vec<u8>>, name: &str, header_size: u64, data: &[u8]) {
    let record = pax("size", &data.len().to_string());
    let mut x = tar::Header::new_gnu();
    x.set_entry_type(tar::EntryType::XHeader);
    x.set_path("././@PaxHeader").unwrap();
    x.set_mode(0o644);
    x.set_size(record.len() as u64);
    x.set_cksum();
    b.append(&x, &record[..]).unwrap();
    let mut h = tar::Header::new_gnu();
    h.set_path(name).unwrap();
    h.set_mode(0o644);
    h.set_size(header_size);
    h.set_cksum();
    b.append(&h, data).unwrap();
}

#[test]
fn a_pax_size_below_the_header_size_sets_the_fence() {
    let (_h, store) = store();
    // The header claims 8 GiB, the PAX record 1 byte. The window after the entry starts past
    // that byte's block, so a long name just past it is refused, as after any 1-byte file.
    let layer = |n: usize| {
        let mut b = tar::Builder::new(Vec::new());
        pax_sized(&mut b, "var/a", 8 << 30, b"a");
        extended(
            &mut b,
            tar::EntryType::GNULongName,
            "v".repeat(n).as_bytes(),
            "var/x",
        );
        b.into_inner().unwrap()
    };
    let at = image_of(
        &store,
        &[layer(MIB - 1024), layer_bytes(&[M::File("usr/a", "A")])],
    );
    assert_eq!(
        files_under(&from_store(&store, &at).unwrap()),
        ["usr", "usr/a"]
    );
    let past = image_of(&store, &[layer(MIB - 1023)]);
    fails_cleanly(&store, &past, rootfs::LIMITS, FENCED);
}

#[test]
fn a_pax_size_over_a_header_size_of_0_extracts() {
    let (_h, store) = store();
    // 2 MiB of data, more than the 1 MiB a header size of 0 would leave the reader.
    let data = vec![b'b'; 2 * MIB];
    let mut b = tar::Builder::new(Vec::new());
    pax_sized(&mut b, "usr/big", 0, &data);
    let digest = image_of(&store, &[b.into_inner().unwrap()]);
    let root = from_store(&store, &digest).unwrap();
    assert_eq!(fs::read(root.join("usr/big")).unwrap(), data);
}

/// Append a GNU sparse member `name` that stores `stored` bytes and expands to `logical`
/// bytes: one data block at the end, a hole before it.
fn sparse(b: &mut tar::Builder<Vec<u8>>, name: &str, stored: u64, logical: u64) {
    let mut h = tar::Header::new_gnu();
    h.set_path(name).unwrap();
    h.set_entry_type(tar::EntryType::GNUSparse);
    h.set_mode(0o644);
    h.set_size(stored);
    let gnu = h.as_gnu_mut().unwrap();
    gnu.set_real_size(logical);
    gnu.sparse[0].set_offset(logical - stored);
    gnu.sparse[0].set_length(stored);
    h.set_cksum();
    b.append(&h, &vec![b's'; stored as usize][..]).unwrap();
}

fn sparse_refused(name: &str) -> String {
    format!("bad image archive: {name} is a GNU sparse file (image archives do not use them)")
}

#[test]
fn a_gnu_sparse_member_is_refused() {
    let (_h, store) = store();
    let mut b = tar::Builder::new(Vec::new());
    sparse(&mut b, "usr/s", 512, 4096);
    let digest = image_of(
        &store,
        &[
            layer_bytes(&[M::File("usr/a", "A")]),
            b.into_inner().unwrap(),
        ],
    );
    fails_cleanly(&store, &digest, rootfs::LIMITS, &sparse_refused("usr/s"));
    // In the outer archive too, before the member the box looks for.
    let mut b = tar::Builder::new(Vec::new());
    sparse(&mut b, "x", 512, 4096);
    let manifest = br#"[{"Layers":["l0/layer.tar"]}]"#;
    let layer = layer_bytes(&[M::File("usr/a", "A")]);
    for (name, data) in [
        ("manifest.json", &manifest[..]),
        ("l0/layer.tar", &layer[..]),
    ] {
        let mut h = tar::Header::new_gnu();
        h.set_path(name).unwrap();
        h.set_mode(0o644);
        h.set_size(data.len() as u64);
        h.set_cksum();
        b.append(&h, data).unwrap();
    }
    let digest = store.put(&b.into_inner().unwrap()).unwrap();
    fails_cleanly(&store, &digest, rootfs::LIMITS, &sparse_refused("x"));
}

#[test]
fn a_sparse_member_cannot_widen_the_fence() {
    let (_h, store) = store();
    // 512 bytes stored that expand to 1 TiB, then a 4 MiB long name.
    let mut b = tar::Builder::new(Vec::new());
    sparse(&mut b, "usr/s", 512, 1 << 40);
    extended(
        &mut b,
        tar::EntryType::GNULongName,
        &vec![b'v'; 4 * MIB],
        "var/x",
    );
    let digest = image_of(&store, &[b.into_inner().unwrap()]);
    // Past the archive's two passes, a budget one byte past the fence: the error shows the
    // long name was never read.
    fails_cleanly(
        &store,
        &digest,
        budget(OUTER + (MIB + 1) as u64),
        &sparse_refused("usr/s"),
    );
}

#[test]
fn a_plain_tar_whose_first_name_starts_like_bzip2_extracts() {
    let (_h, store) = store();
    let digest = image_of(
        &store,
        &[layer_bytes(&[
            M::File("BZh91AY&SY", "x"),
            M::File("usr/a", "A"),
        ])],
    );
    assert_eq!(
        files_under(&from_store(&store, &digest).unwrap()),
        ["usr", "usr/a"]
    );
}

#[test]
fn an_extraction_over_the_write_cap_is_refused_and_publishes_nothing() {
    let (_h, store) = store();
    // Each pass reads two headers, two data blocks and the end block: 5 * 512 = 2560.
    // Written: the folder `usr` the paths imply, `usr/a`, `usr/b`: 3 * 4096 = 12288.
    // 2 * 2560 + 12288 = 17408, and the two passes over the archive.
    let layers = [layer_bytes(&[
        M::File("usr/a", "12345"),
        M::File("usr/b", "678"),
    ])];
    let digest = image_of(&store, &layers);
    over_budget(&store, &digest, OUTER + 17407);
    assert_eq!(fs::read_dir(store.root.join("rootfs")).unwrap().count(), 0);
    assert!(from_store_capped(&store, &digest, budget(OUTER + 17408)).is_ok());
    // The budget counts the whole image, across layers: layer 1 costs 2 * 1536 + 2 * 4096 =
    // 11264, layer 2 costs 2 * 1536 + 4096 = 7168 (the folder exists), 18432 in all. The
    // passes over the archive read its 76-byte manifest and, on the way to `l1/layer.tar`, the
    // 2048 bytes of layer 1.
    let outer = 512 + 76 + 6 * 512 + 2048;
    let two = image_of(
        &store,
        &[
            layer_bytes(&[M::File("usr/a", "1234")]),
            layer_bytes(&[M::File("usr/b", "5678")]),
        ],
    );
    over_budget(&store, &two, outer + 18431);
    assert!(from_store_capped(&store, &two, budget(outer + 18432)).is_ok());
}

#[test]
fn many_tiny_members_of_the_archive_count_against_the_budget() {
    let (_h, store) = store();
    let n = 20_000;
    let manifest = br#"[{"Layers":["l0/layer.tar"]}]"#;
    let layer = layer_bytes(&[M::File("usr/a", "A")]);
    let mut b = tar::Builder::new(Vec::new());
    let mut add = |name: &str, data: &[u8]| {
        let mut h = tar::Header::new_gnu();
        h.set_path(name).unwrap();
        h.set_mode(0o644);
        h.set_size(data.len() as u64);
        h.set_cksum();
        b.append(&h, data).unwrap();
    };
    for i in 0..n {
        add(&format!("f{i}"), b"");
    }
    add("manifest.json", manifest);
    add("l0/layer.tar", &layer);
    let digest = store.put(&b.into_inner().unwrap()).unwrap();
    // The first pass reads every filler's header, the manifest's header and its bytes; the
    // second reads the same headers, the manifest's block and the layer's header. The layer
    // costs 2 * 1536 to read and 2 * 4096 to write (`usr` and `usr/a`).
    let first = 512 * (n + 1) + manifest.len() as u64;
    let second = 512 * (n + 3);
    let cost = first + second + 2 * 1536 + 2 * 4096;
    over_budget(&store, &digest, cost - 1);
    let root = from_store_capped(&store, &digest, budget(cost)).unwrap();
    assert_eq!(files_under(&root), ["usr", "usr/a"]);
}

#[test]
fn the_compressed_bytes_of_a_gzip_layer_count_against_the_budget() {
    let (_h, store) = store();
    // 5000 empty gzip members, about 20 bytes each, decode to nothing; then the tar.
    let empty = gzip(b"");
    let mut layer = empty.repeat(5000);
    layer.extend(gzip(&layer_bytes(&[M::File("usr/a", "A")])));
    assert!(layer.len() > 100_000, "{}", layer.len());
    let digest = image_of(&store, &[layer]);
    // Two passes read the layer's 100 KB; what they decode is a few KiB.
    over_budget(&store, &digest, 64 << 10);
    let root = from_store(&store, &digest).unwrap();
    assert_eq!(fs::read_to_string(root.join("usr/a")).unwrap(), "A");
}

#[test]
fn a_compression_bomb_stops_at_the_cap() {
    let (_h, store) = store();
    let big = "0".repeat(4 << 20);
    let bomb = gzip(&layer_bytes(&[M::File("usr/zeros", &big)]));
    assert!(bomb.len() < 100_000, "{}", bomb.len());
    let digest = image_of(&store, &[bomb]);
    over_budget(&store, &digest, 1 << 20);
    assert_eq!(fs::read_dir(store.root.join("rootfs")).unwrap().count(), 0);
}

#[test]
fn many_empty_entries_are_stopped_by_the_entry_charge() {
    let (_h, store) = store();
    let names: Vec<String> = (0..1000).map(|i| format!("usr/f{i:04}")).collect();
    let members: Vec<M> = names.iter().map(|n| M::File(n, "")).collect();
    let plain = layer_bytes(&members);
    let layer = gzip(&plain);
    let raw = 2 * layer.len() as u64;
    assert!(layer.len() * 20 < plain.len(), "{}", layer.len());
    let digest = image_of(&store, &[layer]);
    // The decoded bytes, 2 * (1000 * 512 + 512) = 1025024, and the compressed bytes both
    // passes read fit in 2 MiB; the 1001 entries written (1000 files and `usr`) at 4096 each
    // do not.
    over_budget(&store, &digest, 2 << 20);
    let cost = OUTER + raw + 1_025_024 + 1001 * 4096;
    over_budget(&store, &digest, cost - 1);
    assert!(from_store_capped(&store, &digest, budget(cost)).is_ok());
}

#[test]
fn fifty_thousand_files_extract_in_linear_time() {
    let (_h, store) = store();
    let names: Vec<String> = (0..50_000).map(|i| format!("usr/f{i:05}")).collect();
    let members: Vec<M> = names.iter().map(|n| M::File(n, "")).collect();
    let digest = image_of(&store, &[layer_bytes(&members)]);
    let t = Instant::now();
    let root = from_store(&store, &digest).unwrap();
    let took = t.elapsed();
    assert_eq!(fs::read_dir(root.join("usr")).unwrap().count(), 50_000);
    // Seconds when each written path forgets only the records under it; many minutes when
    // it scanned every record.
    assert!(took < Duration::from_secs(60), "{took:?}");
}

#[test]
fn a_bomb_of_skipped_members_stops_in_the_whiteout_pass() {
    let (_h, store) = store();
    let big = "0".repeat(4 << 20);
    // The content pass would refuse `etc/p` (its target is outside the kept folders) before
    // it reads `var/zeros`; the budget error shows the whiteout pass stopped the read.
    let bomb = gzip(&layer_bytes(&[
        M::Hard("etc/p", "var/x", "box"),
        M::File("var/zeros", &big),
    ]));
    assert!(bomb.len() < 100_000, "{}", bomb.len());
    let digest = image_of(&store, &[bomb]);
    over_budget(&store, &digest, 1 << 20);
}

#[test]
fn folders_a_deep_path_implies_are_charged_as_entries() {
    let (_h, store) = store();
    let deep = format!("usr/{}f", "d/".repeat(200));
    let mut b = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_gnu();
    h.set_mode(0o644);
    h.set_size(0);
    b.append_data(&mut h, &deep, &b""[..]).unwrap();
    let digest = image_of(&store, &[b.into_inner().unwrap()]);
    // Each pass reads the long-name header and its name block, the file header and the end
    // block: 4 * 512. Written: the file and 201 folders (`usr` and 200 `d`).
    let cost = OUTER + 2 * 4 * 512 + 202 * 4096;
    over_budget(&store, &digest, cost - 1);
    let root = from_store_capped(&store, &digest, budget(cost)).unwrap();
    assert!(root.join(&deep).is_file());
}

#[test]
fn files_keep_only_permission_bits_and_the_box_owns_them() {
    use std::os::unix::fs::MetadataExt;
    let (_h, store) = store();
    let mut b = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_gnu();
    h.set_path("usr/bin/su").unwrap();
    h.set_mode(0o7755);
    h.set_uid(0);
    h.set_gid(0);
    h.set_size(1);
    h.set_cksum();
    b.append(&h, &b"s"[..]).unwrap();
    let digest = image_of(&store, &[b.into_inner().unwrap()]);
    let root = from_store(&store, &digest).unwrap();
    let m = fs::metadata(root.join("usr/bin/su")).unwrap();
    assert_eq!(m.mode() & 0o7777, 0o755);
    assert_eq!(m.uid(), fs::metadata(&root).unwrap().uid());
}
