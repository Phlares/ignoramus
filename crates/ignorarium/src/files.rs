//! Write-once files: atomic publish, no overwrite, no symlink following.
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
pub enum Written {
    Created,
    /// The file already held exactly these bytes.
    Same,
}

#[derive(Debug)]
pub enum WriteOnceError {
    /// The file exists with different bytes.
    Differs,
    /// `path` is a symbolic link.
    Link,
    Io(io::Error),
}

impl std::fmt::Display for WriteOnceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteOnceError::Differs => write!(f, "file exists and differs"),
            WriteOnceError::Link => write!(f, "path is a symbolic link"),
            WriteOnceError::Io(e) => write!(f, "I/O failed: {e}"),
        }
    }
}

impl std::error::Error for WriteOnceError {}

impl From<io::Error> for WriteOnceError {
    fn from(e: io::Error) -> Self {
        WriteOnceError::Io(e)
    }
}

/// Write `bytes` to `path` once. `mode` applies on unix only.
/// The bytes go to a unique temp file in the same directory, then
/// `hard_link` publishes them: it fails if `path` exists, so a racing
/// writer cannot replace a file. The temp file is always removed.
pub fn write_once(path: &Path, bytes: &[u8], mode: u32) -> Result<Written, WriteOnceError> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => return Err(WriteOnceError::Link),
        Ok(_) => return compare(path, bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_string_lossy();
    let mut rnd = [0u8; 8];
    getrandom::fill(&mut rnd).map_err(|e| io::Error::other(e.to_string()))?;
    let tmp = path.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        hex::encode(rnd)
    ));
    let out = publish(&tmp, path, bytes, mode);
    let _ = fs::remove_file(&tmp);
    out
}

fn publish(tmp: &Path, path: &Path, bytes: &[u8], mode: u32) -> Result<Written, WriteOnceError> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, mode);
    #[cfg(not(unix))]
    let _ = mode;
    let mut f = opts.open(tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    match fs::hard_link(tmp, path) {
        Ok(()) => Ok(Written::Created),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => compare(path, bytes),
        Err(e) => Err(e.into()),
    }
}

fn compare(path: &Path, bytes: &[u8]) -> Result<Written, WriteOnceError> {
    if fs::read(path)? == bytes {
        Ok(Written::Same)
    } else {
        Err(WriteOnceError::Differs)
    }
}
