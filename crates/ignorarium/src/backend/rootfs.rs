//! The environment root of a level 1 row: a saved container image, extracted once from the
//! content store into `store/rootfs/<digest>/`. Only the folders a sandbox binds are kept.
use crate::files::temp_beside;
use crate::home::make_private_dir;
use crate::store::{Store, StoreError};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

/// The folders of an image that level 1 binds into the sandbox.
const KEPT: [&str; 5] = ["usr", "bin", "lib", "lib64", "etc"];
const WHITEOUT: &str = ".wh.";
const OPAQUE: &str = ".wh..wh..opq";
/// Longest `tar` error text kept.
const TAIL: usize = 2000;

#[derive(Debug)]
pub enum RootfsError {
    /// The digest is not in the store.
    Missing(String),
    Store(StoreError),
    /// The archive or a layer is not what the image format promises.
    Format(String),
    /// A layer writes through a symbolic link, or names a path outside its tree.
    Unsafe(String),
    /// `tar` failed.
    Tar(String),
    Io(io::Error),
}

impl fmt::Display for RootfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RootfsError::Missing(d) => write!(f, "image {d} is not in the store"),
            RootfsError::Store(e) => write!(f, "store: {e}"),
            RootfsError::Format(m) => write!(f, "bad image archive: {m}"),
            RootfsError::Unsafe(m) => write!(f, "unsafe image layer: {m}"),
            RootfsError::Tar(m) => write!(f, "tar failed: {m}"),
            RootfsError::Io(e) => write!(f, "I/O failed: {e}"),
        }
    }
}

impl std::error::Error for RootfsError {}

impl From<io::Error> for RootfsError {
    fn from(e: io::Error) -> Self {
        RootfsError::Io(e)
    }
}

impl From<StoreError> for RootfsError {
    fn from(e: StoreError) -> Self {
        RootfsError::Store(e)
    }
}

#[derive(Deserialize)]
struct ManifestEntry {
    #[serde(rename = "Layers")]
    layers: Vec<String>,
}

/// The root of the image saved under `store_digest`, extracted on the first call and reused
/// after. A crash leaves a temp folder beside the result, never a partial tree at it.
pub fn from_store(store: &Store, store_digest: &str) -> Result<PathBuf, RootfsError> {
    let blob = store.path(store_digest)?;
    if !store.has(store_digest) {
        return Err(RootfsError::Missing(store_digest.into()));
    }
    let parent = store.root.join("rootfs");
    make_private_dir(&parent, true, false)
        .map_err(|e| RootfsError::Io(io::Error::other(e.to_string())))?;
    let done = parent.join(store_digest);
    if is_real_dir(&done) {
        return Ok(done);
    }
    let tmp = temp_beside(&done)?;
    fs::create_dir(&tmp)?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o700))?;
    let built = extract(&blob, &tmp).and_then(|()| match fs::rename(tmp.join("root"), &done) {
        Ok(()) => Ok(()),
        // Another box published the same tree first.
        Err(_) if is_real_dir(&done) => Ok(()),
        Err(e) => Err(e.into()),
    });
    // The temp tree is this call's own scratch; layers may leave folders unwritable.
    make_writable(&tmp);
    let _ = fs::remove_dir_all(&tmp);
    built.map(|()| done)
}

fn is_real_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// `tar -xf blob -C dir` of the named members only.
fn extract_members(blob: &Path, dir: &Path, members: &[&str]) -> Result<(), RootfsError> {
    let mut cmd = Command::new("tar");
    cmd.arg("-xf")
        .arg(blob)
        .arg("-C")
        .arg(dir)
        .args(["--no-same-owner", "--no-wildcards", "--"])
        .args(members);
    run(&mut cmd).map(|_| ())
}

/// Extract the image at `blob` into `work/root`, using `work/image` for the layer files.
fn extract(blob: &Path, work: &Path) -> Result<(), RootfsError> {
    let (image, root) = (work.join("image"), work.join("root"));
    fs::create_dir(&image)?;
    fs::create_dir(&root)?;
    extract_members(blob, &image, &["manifest.json"])?;
    let text = fs::read(image.join("manifest.json"))?;
    let manifest: Vec<ManifestEntry> = serde_json::from_slice(&text)
        .map_err(|e| RootfsError::Format(format!("manifest.json: {e}")))?;
    let layers = manifest
        .into_iter()
        .next()
        .map(|m| m.layers)
        .filter(|l| !l.is_empty())
        .ok_or_else(|| RootfsError::Format("manifest.json lists no layers".into()))?;
    if let Some(bad) = layers.iter().find(|l| !plain_path(l)) {
        return Err(RootfsError::Unsafe(format!("layer path {bad:?}")));
    }
    let unique: BTreeSet<&str> = layers.iter().map(String::as_str).collect();
    extract_members(blob, &image, &unique.into_iter().collect::<Vec<_>>())?;
    for l in &layers {
        let layer = image.join(l);
        if !fs::symlink_metadata(&layer)?.is_file() {
            return Err(RootfsError::Format(format!("layer {l} is not a file")));
        }
        apply_layer(&layer, &root)?;
    }
    Ok(())
}

/// True for a relative path of normal components.
fn plain_path(p: &str) -> bool {
    Path::new(p)
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
        && !p.is_empty()
}

/// A member name without leading `./` or `/` and without a trailing `/`.
fn clean(name: &str) -> &str {
    name.trim_start_matches("./")
        .trim_start_matches('/')
        .trim_end_matches('/')
}

fn run(cmd: &mut Command) -> Result<Vec<u8>, RootfsError> {
    let out = cmd.stdin(Stdio::null()).output()?;
    if out.status.success() {
        return Ok(out.stdout);
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let start = err.char_indices().rev().nth(TAIL - 1).map_or(0, |(i, _)| i);
    Err(RootfsError::Tar(err[start..].trim().to_string()))
}

/// The lines `tar` prints for the layer: names only, or one detail line per member.
fn listing(layer: &Path, verbose: bool) -> Result<Vec<String>, RootfsError> {
    let mut cmd = Command::new("tar");
    cmd.arg(if verbose { "-tvf" } else { "-tf" })
        .arg(layer)
        .arg("--quoting-style=literal");
    let out = run(&mut cmd)?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .map(String::from)
        .collect())
}

/// Delete what the layer's whiteouts name from `root`, then extract the layer's members
/// that fall in the kept folders, except device nodes, pipes and whiteout markers.
fn apply_layer(layer: &Path, root: &Path) -> Result<(), RootfsError> {
    let (names, details) = (listing(layer, false)?, listing(layer, true)?);
    if names.len() != details.len() {
        return Err(RootfsError::Format(
            "a member name spans several lines".into(),
        ));
    }
    make_writable(root);
    for name in &names {
        whiteout(root, clean(name))?;
    }
    let members: Vec<(&String, &str, char)> = names
        .iter()
        .zip(&details)
        .map(|(n, d)| (n, clean(n), d.chars().next().unwrap_or('-')))
        .collect();
    // Every link of the layer counts, wherever it falls in the order.
    let links: BTreeSet<String> = members
        .iter()
        .filter(|(_, _, kind)| *kind == 'l')
        .map(|(_, path, _)| path.to_string())
        .collect();
    let mut keep: Vec<u8> = Vec::new();
    for (name, path, kind) in members {
        if !kept(path)? || matches!(kind, 'c' | 'b' | 'p') {
            continue;
        }
        through_link(root, &links, path)?;
        keep.extend_from_slice(name.as_bytes());
        keep.push(0);
    }
    if keep.is_empty() {
        return Ok(());
    }
    let list = layer.with_extension("keep");
    fs::write(&list, &keep)?;
    let mut cmd = Command::new("tar");
    cmd.arg("-xf")
        .arg(layer)
        .arg("-C")
        .arg(root)
        .args(["--no-same-owner", "--no-recursion", "--no-wildcards"])
        .args(["--null", "-T"])
        .arg(&list);
    run(&mut cmd).map(|_| ())
}

/// True when the member lies in a kept folder and is not a whiteout marker. A path with
/// `..` or other odd parts is an error.
fn kept(path: &str) -> Result<bool, RootfsError> {
    if path.is_empty() || path == "." {
        return Ok(false);
    }
    if !plain_path(path) {
        return Err(RootfsError::Unsafe(format!("member path {path:?}")));
    }
    let first = path.split('/').next().unwrap_or_default();
    let base = path.rsplit('/').next().unwrap_or_default();
    Ok(KEPT.contains(&first) && !base.starts_with(WHITEOUT))
}

/// Refuse a member whose parent folders include a link, in the tree or earlier in its layer.
fn through_link(root: &Path, links: &BTreeSet<String>, path: &str) -> Result<(), RootfsError> {
    let parts: Vec<&str> = path.split('/').collect();
    for k in 1..parts.len() {
        let prefix = parts[..k].join("/");
        let on_disk = fs::symlink_metadata(root.join(&prefix)).is_ok_and(|m| m.is_symlink());
        if on_disk || links.contains(&prefix) {
            return Err(RootfsError::Unsafe(format!(
                "{path} would be written through the link {prefix}"
            )));
        }
    }
    Ok(())
}

/// Apply the whiteout `path` names, if it is one: `.wh.<name>` deletes `<name>` beside it;
/// `.wh..wh..opq` empties its folder.
fn whiteout(root: &Path, path: &str) -> Result<(), RootfsError> {
    let Some((dir, base)) = path.rsplit_once('/') else {
        return Ok(());
    };
    let first = dir.split('/').next().unwrap_or_default();
    if !base.starts_with(WHITEOUT) || !KEPT.contains(&first) {
        return Ok(());
    }
    if !plain_path(path) {
        return Err(RootfsError::Unsafe(format!("whiteout path {path:?}")));
    }
    through_link(root, &BTreeSet::new(), path)?;
    let folder = root.join(dir);
    if base == OPAQUE {
        let Ok(entries) = fs::read_dir(&folder) else {
            return Ok(());
        };
        for e in entries {
            remove(&e?.path())?;
        }
        Ok(())
    } else {
        remove(&folder.join(&base[WHITEOUT.len()..]))
    }
}

/// Remove a file, a link (not its target) or a folder tree; absent is fine.
fn remove(p: &Path) -> Result<(), RootfsError> {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => {
            make_writable(p);
            Ok(fs::remove_dir_all(p)?)
        }
        Ok(_) => Ok(fs::remove_file(p)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Give every folder under `p` (not following links) the owner's read, write and search bits.
fn make_writable(p: &Path) {
    let Ok(m) = fs::symlink_metadata(p) else {
        return;
    };
    if !m.is_dir() {
        return;
    }
    let mode = m.permissions().mode() | 0o700;
    let _ = fs::set_permissions(p, fs::Permissions::from_mode(mode));
    if let Ok(entries) = fs::read_dir(p) {
        for e in entries.flatten() {
            make_writable(&e.path());
        }
    }
}
