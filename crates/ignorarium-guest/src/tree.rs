//! Directory trees: one walk, and the output hash, SHA-256 of the rows
//! `[relpath, kind, digest]` sorted as whole rows. A file's digest is the SHA-256 of its
//! bytes and a link's the SHA-256 of its target bytes; other kinds have none.
use sha2::{Digest, Sha256};
use std::fs::{self, File, FileType, OpenOptions};
use std::io::{self, Read};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// One entry below a folder: its path as text, `file`, `link` or `other`, and its digest.
/// Folders have no row.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TreeRow {
    pub path: String,
    pub kind: &'static str,
    pub digest: String,
}

/// The row of the entry at `rel`, the `/`-separated path bytes below the folder.
pub fn row(rel: &[u8], kind: &'static str, digest: String) -> TreeRow {
    TreeRow {
        path: rel_text(rel),
        kind,
        digest,
    }
}

/// A link's row. The target is never followed: the row records where the link points.
pub fn link_row(rel: &[u8], target: &[u8]) -> TreeRow {
    row(rel, "link", hex::encode(Sha256::digest(target)))
}

/// The rows of every entry below `dir`, sorted. A linked `dir` is an error.
pub fn tree_rows(dir: &Path) -> io::Result<Vec<TreeRow>> {
    if fs::symlink_metadata(dir)?.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "top dir is a link",
        ));
    }
    let mut rows = Vec::new();
    walk(dir, &mut |rel, path, ty| {
        rows.push(match Entry::of(path, ty)? {
            Entry::Dir => return Ok(()),
            Entry::Link(target) => link_row(rel, &target),
            Entry::File(f) => row(rel, "file", sha256_hex(f)?),
            Entry::Other => row(rel, "other", String::new()),
        });
        Ok(())
    })?;
    rows.sort();
    Ok(rows)
}

/// SHA-256 of the rows as compact JSON, taken in sorted order whatever order they come in.
/// The JSON goes straight into the hasher, so no copy of it is held.
pub fn hash_rows(rows: &[TreeRow]) -> io::Result<String> {
    let mut rows: Vec<_> = rows.iter().map(|r| (&r.path, r.kind, &r.digest)).collect();
    rows.sort();
    let mut h = Sha256::new();
    serde_json::to_writer(&mut h, &rows)?;
    Ok(hex::encode(h.finalize()))
}

pub fn tree_hash(dir: &Path) -> io::Result<String> {
    hash_rows(&tree_rows(dir)?)
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

/// What a walked entry is, read without following a link. The hash and the pack both decide
/// an entry's kind here, so they cannot disagree.
pub(crate) enum Entry {
    Dir,
    /// The target bytes.
    Link(Vec<u8>),
    /// Open for reading.
    File(File),
    Other,
}

impl Entry {
    /// The entry at `path`, which the walk listed as `ty`. A file that stopped being a regular
    /// file after the listing is `Other`.
    pub(crate) fn of(path: &Path, ty: FileType) -> io::Result<Entry> {
        Ok(if ty.is_dir() {
            Entry::Dir
        } else if ty.is_symlink() {
            Entry::Link(fs::read_link(path)?.into_os_string().into_vec())
        } else if ty.is_file()
            && let Some(f) = open_regular(path)?
        {
            Entry::File(f)
        } else {
            Entry::Other
        })
    }
}

/// `None` when the path stopped being a regular file between listing and open.
fn open_regular(path: &Path) -> io::Result<Option<File>> {
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
