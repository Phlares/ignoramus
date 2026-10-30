//! The payload hash: every file under the payload folder, so code that `payload/run` calls is
//! pinned too. `snapshot` copies the folder into the box; runs use that copy.
use crate::backend::output::{hash_rows, open_regular, tree_rows};
use crate::files::{self, WriteOnceError};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// The payload folder, below a study root and below a study folder in the box.
pub const DIR: &str = "payload";

#[derive(Debug)]
pub enum PayloadError {
    /// The folder has no regular file `run`.
    NoRun,
    /// A link or a non-regular file, by path. A linked payload folder names itself.
    Unsafe(String),
    /// A file changed while the folder was copied: the copy is not the folder that was hashed.
    Changed,
    Io(io::Error),
}

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PayloadError::NoRun => write!(f, "the payload folder has no run file"),
            PayloadError::Unsafe(p) => write!(f, "{p} is a link or not a regular file"),
            PayloadError::Changed => write!(f, "the payload folder changed while it was copied"),
            PayloadError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PayloadError {}

impl From<io::Error> for PayloadError {
    fn from(e: io::Error) -> Self {
        PayloadError::Io(e)
    }
}

/// SHA-256 over the sorted rows of every file below `dir`.
pub fn tree_hash(dir: &Path) -> Result<String, PayloadError> {
    if std::fs::symlink_metadata(dir)?.file_type().is_symlink() {
        return Err(PayloadError::Unsafe(dir.display().to_string()));
    }
    let rows = tree_rows(dir)?;
    if let Some(r) = rows.iter().find(|r| r.kind != "file") {
        return Err(PayloadError::Unsafe(
            dir.join(&r.path).display().to_string(),
        ));
    }
    if !rows.iter().any(|r| r.path == "run") {
        return Err(PayloadError::NoRun);
    }
    Ok(hash_rows(&rows)?)
}

/// Copy `root/payload` to `dest/payload` and return the copy's tree hash, which equals the
/// source's. Each file is written once, mode 0o555 when the source has an exec bit, else
/// 0o444; then every copied folder is set to 0o555. The pinned code then cannot change under
/// a run, and a level 0 payload cannot leave files (a Python cache) beside it. The tree hash
/// records no modes.
pub fn snapshot(root: &Path, dest: &Path) -> Result<String, PayloadError> {
    let want = tree_hash(&root.join(DIR))?;
    let copy = dest.join(DIR);
    copy_tree(&root.join(DIR), &copy)?;
    files::seal(&copy)?;
    if tree_hash(&copy)? != want {
        return Err(PayloadError::Changed);
    }
    Ok(want)
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), PayloadError> {
    fs::create_dir(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if e.file_type()?.is_dir() {
            copy_tree(&src, &dst)?;
            continue;
        }
        let unsafe_at = || PayloadError::Unsafe(src.display().to_string());
        let mut f = open_regular(&src)?.ok_or_else(unsafe_at)?;
        let exec = f.metadata()?.permissions().mode() & 0o111 != 0;
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes)?;
        files::write_once(&dst, &bytes, if exec { 0o555 } else { 0o444 }).map_err(|e| match e {
            WriteOnceError::Io(e) => PayloadError::Io(e),
            WriteOnceError::Link | WriteOnceError::Differs => {
                PayloadError::Unsafe(dst.display().to_string())
            }
        })?;
    }
    Ok(())
}
