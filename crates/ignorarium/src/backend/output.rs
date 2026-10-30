//! The hash of a folder, the same at every isolation level: a row's output and the payload.
use crate::canon::{sha256_hex, to_canonical};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// One entry of a folder: a path below it, `file`, `link` or `other`, and a file's SHA-256.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TreeRow {
    pub path: String,
    pub kind: &'static str,
    pub digest: String,
}

/// The sorted rows under `dir`. Links are recorded, never followed. A symlinked `dir` is refused.
pub fn tree_rows(dir: &Path) -> io::Result<Vec<TreeRow>> {
    if fs::symlink_metadata(dir)?.file_type().is_symlink() {
        return Err(io::Error::other("output dir is a symbolic link"));
    }
    let mut rows = Vec::new();
    walk(dir, "", &mut rows)?;
    rows.sort();
    Ok(rows)
}

/// Hash of the canonical `[relpath, kind, digest]` rows.
pub fn hash_rows(rows: &[TreeRow]) -> io::Result<String> {
    let v = Value::Array(
        rows.iter()
            .map(|r| json!([r.path, r.kind, r.digest]))
            .collect(),
    );
    let bytes = to_canonical(&v).map_err(io::Error::other)?;
    Ok(sha256_hex(&bytes))
}

/// Hash of the sorted rows under `dir`.
pub fn output_hash(dir: &Path) -> io::Result<String> {
    hash_rows(&tree_rows(dir)?)
}

/// A name as text; bytes that are not UTF-8 become `!bytes:<hex>`.
fn name_of(n: &OsStr) -> String {
    match n.to_str() {
        Some(s) => s.to_string(),
        None => format!("!bytes:{}", hex::encode(n.as_bytes())),
    }
}

fn walk(dir: &Path, prefix: &str, rows: &mut Vec<TreeRow>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = format!("{prefix}{}", name_of(&entry.file_name()));
        let row = |kind, digest| TreeRow {
            path: path.clone(),
            kind,
            digest,
        };
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk(&entry.path(), &format!("{path}/"), rows)?;
        } else if ft.is_symlink() {
            rows.push(row("link", String::new()));
        } else if ft.is_file() {
            rows.push(match file_digest(&entry.path())? {
                Some(d) => row("file", d),
                None => row("other", String::new()),
            });
        } else {
            rows.push(row("other", String::new()));
        }
    }
    Ok(())
}

/// SHA-256 of a regular file, streamed. `None` when the path is no longer a
/// regular file by the time it is open (swapped for a FIFO or a link).
fn file_digest(path: &Path) -> io::Result<Option<String>> {
    let opened = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path);
    let mut f = match opened {
        Ok(f) => f,
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return Ok(None),
        Err(e) => return Err(e),
    };
    if !f.metadata()?.is_file() {
        return Ok(None);
    }
    let mut h = Sha256::new();
    io::copy(&mut f, &mut h)?;
    Ok(Some(hex::encode(h.finalize())))
}
