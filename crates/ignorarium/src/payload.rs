//! The payload hash: every file under the payload folder, so code that `payload/run` calls is pinned too.
use crate::backend::output::{hash_rows, tree_rows};
use std::fmt;
use std::io;
use std::path::Path;

#[derive(Debug)]
pub enum PayloadError {
    /// The folder has no regular file `run`.
    NoRun,
    /// A link or a non-regular file, by path. A linked payload folder names itself.
    Unsafe(String),
    Io(io::Error),
}

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PayloadError::NoRun => write!(f, "the payload folder has no run file"),
            PayloadError::Unsafe(p) => write!(f, "{p} is a link or not a regular file"),
            PayloadError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PayloadError {}

/// SHA-256 over the sorted rows of every file below `dir`.
pub fn tree_hash(dir: &Path) -> Result<String, PayloadError> {
    if std::fs::symlink_metadata(dir)
        .map_err(PayloadError::Io)?
        .file_type()
        .is_symlink()
    {
        return Err(PayloadError::Unsafe(dir.display().to_string()));
    }
    let rows = tree_rows(dir).map_err(PayloadError::Io)?;
    if let Some(r) = rows.iter().find(|r| r.kind != "file") {
        return Err(PayloadError::Unsafe(
            dir.join(&r.path).display().to_string(),
        ));
    }
    if !rows.iter().any(|r| r.path == "run") {
        return Err(PayloadError::NoRun);
    }
    hash_rows(&rows).map_err(PayloadError::Io)
}
