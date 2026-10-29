//! Directory trees: one walk, and the output hash, SHA-256 of the rows
//! `[relpath, kind, digest]` sorted as whole rows.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File, FileType, OpenOptions};
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub fn tree_hash(dir: &Path) -> io::Result<String> {
    if fs::symlink_metadata(dir)?.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "top dir is a link",
        ));
    }
    let mut rows: Vec<(String, &str, String)> = Vec::new();
    walk(dir, &mut |rel, path, ty| {
        let (kind, digest) = if ty.is_dir() {
            return Ok(());
        } else if ty.is_symlink() {
            ("link", String::new())
        } else if ty.is_file()
            && let Some(f) = open_regular(path)?
        {
            ("file", sha256_hex(f)?)
        } else {
            ("other", String::new())
        };
        rows.push((rel_text(rel), kind, digest));
        Ok(())
    })?;
    rows.sort();
    let rows: Vec<Value> = rows.into_iter().map(|(p, k, d)| json!([p, k, d])).collect();
    let bytes = serde_json::to_vec(&Value::Array(rows))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

/// `rel` as text, part by part; a part that is not UTF-8 becomes `!bytes:<hex>`.
fn rel_text(rel: &[u8]) -> String {
    let parts: Vec<String> = rel
        .split(|&b| b == b'/')
        .map(|p| match std::str::from_utf8(p) {
            Ok(s) => s.to_string(),
            Err(_) => format!("!bytes:{}", hex::encode(p)),
        })
        .collect();
    parts.join("/")
}

/// Calls `visit` for every entry below `dir`, depth first: names in byte order, a directory
/// before what it holds. `visit` gets the path below `dir` (`/`-separated bytes), the full
/// path and the type. Links are not followed.
pub(crate) fn walk(
    dir: &Path,
    visit: &mut impl FnMut(&[u8], &Path, FileType) -> io::Result<()>,
) -> io::Result<()> {
    walk_below(dir, b"", visit)
}

fn walk_below(
    dir: &Path,
    prefix: &[u8],
    visit: &mut impl FnMut(&[u8], &Path, FileType) -> io::Result<()>,
) -> io::Result<()> {
    let mut entries = fs::read_dir(dir)?
        .map(|e| e.and_then(|e| Ok((e.file_name(), e.file_type()?))))
        .collect::<io::Result<Vec<_>>>()?;
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    for (name, ty) in entries {
        let rel = [prefix, name.as_bytes()].concat();
        let path = dir.join(&name);
        visit(&rel, &path, ty)?;
        if ty.is_dir() {
            walk_below(&path, &[&rel[..], b"/"].concat(), visit)?;
        }
    }
    Ok(())
}

/// `None` when the path stopped being a regular file between listing and open.
pub(crate) fn open_regular(path: &Path) -> io::Result<Option<File>> {
    let f = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(f.metadata()?.file_type().is_file().then_some(f))
}

pub(crate) fn sha256_hex(mut r: impl Read) -> io::Result<String> {
    let mut h = Sha256::new();
    io::copy(&mut r, &mut h)?;
    Ok(hex::encode(h.finalize()))
}
