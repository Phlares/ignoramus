//! Where an input comes from: the content store when the spec pins a stored digest, else
//! the file under the study root. Every reader of an input goes through this module:
//! `input_path` to guard its path, `resolve` to hash it in place, `stored` and
//! `regular_file` to bring it into the store. `study_root` finds the folder every input path
//! is relative to.
use crate::prior::file_hashes;
use crate::store::{Meta, Store, StoreError};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// The store holds the blob; the digest is its pin.
    Stored { digest: String, meta: Meta },
    /// A regular file under the study root, with its hashes.
    File { exact: String, normalized: String },
    /// The path is safe and names no regular file.
    Missing,
    /// The path could leave the study root.
    Unsafe,
    /// The store has a record for the pin, but it cannot be read, or its blob is gone or
    /// has another size. The text says which.
    Damaged(String),
}

/// `rel` under `root`, or `None` when it could leave the folder. Every
/// segment must be one plain name; empty segments (`a//b`) are skipped. A `:`, a
/// backslash, a leading `/`, a segment ending in `.` or a space (Windows trims
/// them), or any symlink on the way makes it unsafe.
pub fn input_path(root: &Path, rel: &str) -> Option<PathBuf> {
    if rel.starts_with('/') || rel.contains(['\\', ':']) {
        return None;
    }
    let mut path = root.to_path_buf();
    let mut any = false;
    for part in rel.split('/').filter(|c| !c.is_empty()) {
        let mut parts = Path::new(part).components();
        // One Normal component per segment: no `..`, root or prefix, so the result stays inside.
        let plain = matches!(
            (parts.next(), parts.next()),
            (Some(Component::Normal(_)), None)
        );
        if !plain || part.ends_with(['.', ' ']) {
            return None;
        }
        path.push(part);
        any = true;
        if path.symlink_metadata().is_ok_and(|m| m.is_symlink()) {
            return None;
        }
    }
    any.then_some(path)
}

/// The bytes of a regular file; never opens a pipe or device.
pub fn read_regular(path: &Path) -> Option<Vec<u8>> {
    path.metadata()
        .ok()
        .filter(std::fs::Metadata::is_file)
        .and_then(|_| std::fs::read(path).ok())
}

/// `Stored` when `sha256` is set and the store has its meta and a blob of that size
/// (a stat, not a read: the bytes are hashed at import and when a study registers); else
/// the file at `path`.
pub fn resolve(store: &Store, root: &Path, path: &str, sha256: &str) -> Found {
    if let Some(found) = stored(store, sha256) {
        return found;
    }
    let Some(file) = input_path(root, path) else {
        return Found::Unsafe;
    };
    match read_regular(&file) {
        Some(bytes) => {
            let (exact, normalized) = file_hashes(&bytes);
            Found::File { exact, normalized }
        }
        None => Found::Missing,
    }
}

/// `Stored` or `Damaged` when the store has a record for `sha256`; `None` when the pin is
/// empty, not a digest, or unknown to the store.
pub fn stored(store: &Store, sha256: &str) -> Option<Found> {
    if sha256.is_empty() {
        return None;
    }
    match store.meta(sha256) {
        Ok(Some(meta)) => Some(held(store, sha256, meta)),
        Ok(None) | Err(StoreError::BadDigest) => None,
        Err(e) => Some(Found::Damaged(format!(
            "the store's record cannot be read: {e}"
        ))),
    }
}

/// The regular file `path` names under `root`, without reading it; `None` when the path is
/// unsafe or names no regular file.
pub fn regular_file(root: &Path, path: &str) -> Option<PathBuf> {
    input_path(root, path).filter(|p| p.metadata().is_ok_and(|m| m.is_file()))
}

fn held(store: &Store, digest: &str, meta: Meta) -> Found {
    let blob = store
        .path(digest)
        .ok()
        .and_then(|p| p.symlink_metadata().ok());
    match blob {
        Some(m) if m.is_file() && m.len() == meta.size => Found::Stored {
            digest: digest.to_string(),
            meta,
        },
        _ => Found::Damaged("the stored object is missing or has another size".into()),
    }
}

/// `(exact, normalized)` SHA-256 of the input, or `None` when there is none to read.
pub fn hashes(found: &Found) -> Option<(String, String)> {
    match found {
        Found::Stored { digest, meta } => Some((digest.clone(), meta.normalized_sha256.clone())),
        Found::File { exact, normalized } => Some((exact.clone(), normalized.clone())),
        Found::Missing | Found::Unsafe | Found::Damaged(_) => None,
    }
}

/// Each input path with its `(exact, normalized)` hashes, in spec order; `None` while the
/// input is absent.
pub type InputHashes = Vec<(String, Option<(String, String)>)>;

/// The hashes of the data of the spec's input table `input`, from the store when its pin
/// names a stored object; `None` when it has no path or there is nothing to read.
pub fn hashes_of(store: &Store, root: &Path, input: &Value) -> Option<(String, String)> {
    let path = input.get("path")?.as_str()?;
    let sha = input.get("sha256").and_then(Value::as_str).unwrap_or("");
    hashes(&resolve(store, root, path, sha))
}

/// `hashes_of` every input the spec lists with a path. This is the one read `register`
/// relies on.
pub fn spec_hashes(store: &Store, root: &Path, spec: &Value) -> InputHashes {
    let list = spec["inputs"].as_array().map_or(&[][..], Vec::as_slice);
    (list.iter())
        .filter_map(|i| Some((i["path"].as_str()?.to_string(), hashes_of(store, root, i))))
        .collect()
}

/// The file whose presence marks a study root.
pub const ROOT_MARKER: &str = "ignorarium.toml";

/// The folder every study path is relative to: the nearest folder at or above the spec's
/// own folder that holds a regular `ignorarium.toml` (a link or folder of that name does
/// not count; the content is not read). The walk checks the first folder that holds a
/// `.git` entry and goes no further. It never uses a marker in the user's home folder or
/// the filesystem root. Without a marker, the spec's folder.
pub fn study_root(spec_path: &Path) -> PathBuf {
    study_root_in(spec_path, std::env::home_dir().as_deref())
}

/// `study_root` for a given home folder.
pub fn study_root_in(spec_path: &Path, home: Option<&Path>) -> PathBuf {
    let spec = std::path::absolute(spec_path).unwrap_or_else(|_| spec_path.to_path_buf());
    let spec = fold_dots(&spec);
    let folder = spec
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    root_at(&folder, home)
}

/// The study root a `run` or `resume` reads late inputs from: `study_root`'s walk from the
/// current folder, which is the root itself when no marker is found.
pub fn current_root() -> std::io::Result<PathBuf> {
    Ok(root_at(
        &std::env::current_dir()?,
        std::env::home_dir().as_deref(),
    ))
}

/// The walk of `study_root` from the absolute folder `folder`.
fn root_at(folder: &Path, home: Option<&Path>) -> PathBuf {
    for dir in folder.ancestors() {
        if dir.parent().is_none() || Some(dir) == home {
            break;
        }
        let has = |name: &str| dir.join(name).symlink_metadata();
        if has(ROOT_MARKER).is_ok_and(|m| m.is_file()) {
            return dir.to_path_buf();
        }
        if has(".git").is_ok() {
            break;
        }
    }
    folder.to_path_buf()
}

/// Drop `.` and fold `..` against the segment before it, without touching the disk, so the
/// walk climbs real parents only.
fn fold_dots(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}
