//! Write-once files: atomic publish, no overwrite, no symlink following.
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
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

/// Open an append-only log for reading and appending, creating it if
/// absent. Fails if `path` is a symbolic link, or (unix) if the opened
/// file is not the one `path` named before the open.
pub fn open_log(path: &Path) -> io::Result<fs::File> {
    let before = match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            return Err(io::Error::other("log path is a symbolic link"));
        }
        Ok(m) => Some(m),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let mut opts = OpenOptions::new();
    opts.read(true).append(true).create(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let f = opts.open(path)?;
    #[cfg(unix)]
    if let Some(b) = before {
        use std::os::unix::fs::MetadataExt;
        let a = f.metadata()?;
        if (a.dev(), a.ino()) != (b.dev(), b.ino()) {
            return Err(io::Error::other("log file changed during open"));
        }
    }
    #[cfg(not(unix))]
    let _ = before;
    Ok(f)
}

/// Bytes read per step when scanning back from the end of a log.
pub const TAIL_CHUNK: usize = 4096;

/// The end of a log file.
#[derive(Debug)]
pub enum Tail {
    Empty,
    /// The last line, including its final LF.
    Line(Vec<u8>),
    /// Bytes after the last LF: the file does not end with LF.
    Partial(Vec<u8>),
}

/// Read only the last line of `r`, scanning back from the end.
pub fn read_tail<R: Read + Seek>(r: &mut R) -> io::Result<Tail> {
    let len = r.seek(SeekFrom::End(0))?;
    if len == 0 {
        return Ok(Tail::Empty);
    }
    let mut buf: Vec<u8> = Vec::new();
    let mut pos = len;
    let mut ends_with_lf = false;
    let mut first = true;
    loop {
        let start = pos.saturating_sub(TAIL_CHUNK as u64);
        let mut chunk = vec![0u8; (pos - start) as usize];
        r.seek(SeekFrom::Start(start))?;
        r.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&buf);
        buf = chunk;
        pos = start;
        if first {
            ends_with_lf = buf.last() == Some(&b'\n');
            first = false;
        }
        let limit = buf.len() - usize::from(ends_with_lf);
        if let Some(i) = buf[..limit].iter().rposition(|&b| b == b'\n') {
            buf.drain(..=i);
            break;
        }
        if pos == 0 {
            break;
        }
    }
    Ok(if ends_with_lf {
        Tail::Line(buf)
    } else {
        Tail::Partial(buf)
    })
}

/// Append one write to a log under an exclusive lock. `build` sees the
/// log's tail and returns the bytes to write; they go out in one
/// `write_all`, then fsync.
pub fn locked_append<E: From<io::Error>>(
    path: &Path,
    build: impl FnOnce(Tail) -> Result<Vec<u8>, E>,
) -> Result<(), E> {
    let mut f = open_log(path)?;
    f.lock()?;
    let bytes = build(read_tail(&mut f)?)?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    Ok(())
}

/// Replace `path` with `bytes` atomically: a unique temp file in the same
/// directory, fsync, then `rename` over `path`. Refuses a symbolic link at
/// `path`. The temp file is removed on any failure. `mode` applies on unix only.
pub fn replace_atomic(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path is a symbolic link",
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
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
    let out = stage(&tmp, bytes, mode).and_then(|()| fs::rename(&tmp, path));
    if out.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    out
}

fn stage(tmp: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, mode);
    #[cfg(not(unix))]
    let _ = mode;
    let mut f = opts.open(tmp)?;
    f.write_all(bytes)?;
    f.sync_all()
}
