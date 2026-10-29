//! Output-tree hash: SHA-256 of the sorted rows `[relpath, kind, digest]`.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

type Row = (String, &'static str, String);

pub fn tree_hash(dir: &Path) -> io::Result<String> {
    if fs::symlink_metadata(dir)?.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "top dir is a link",
        ));
    }
    let mut rows = Vec::new();
    walk(dir, "", &mut rows)?;
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let rows: Vec<Value> = rows.into_iter().map(|(p, k, d)| json!([p, k, d])).collect();
    let bytes = serde_json::to_vec(&Value::Array(rows))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn name_text(n: &OsStr) -> String {
    match n.to_str() {
        Some(s) => s.to_string(),
        None => format!("!bytes:{}", hex::encode(n.as_bytes())),
    }
}

fn walk(dir: &Path, prefix: &str, rows: &mut Vec<Row>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let rel = format!("{prefix}{}", name_text(&entry.file_name()));
        let ty = entry.file_type()?;
        if ty.is_dir() {
            walk(&entry.path(), &format!("{rel}/"), rows)?;
        } else if ty.is_symlink() {
            rows.push((rel, "link", String::new()));
        } else if ty.is_file() {
            match open_regular(&entry.path())? {
                Some(f) => rows.push((rel, "file", digest(f)?)),
                None => rows.push((rel, "other", String::new())),
            }
        } else {
            rows.push((rel, "other", String::new()));
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

fn digest(mut f: File) -> io::Result<String> {
    let mut h = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(hex::encode(h.finalize()));
        }
        h.update(&buf[..n]);
    }
}
