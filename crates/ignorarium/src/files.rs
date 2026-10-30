//! Write-once files: atomic publish, no overwrite, no symlink following.
use crate::canon::canonical_line;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

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
    let tmp = temp_beside(path)?;
    let out = publish(&tmp, path, bytes, mode);
    let _ = fs::remove_file(&tmp);
    out
}

fn publish(tmp: &Path, path: &Path, bytes: &[u8], mode: u32) -> Result<Written, WriteOnceError> {
    let mut f = create_new(tmp, mode)?;
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

/// Every byte of `r`, from its start.
pub fn read_whole<R: Read + Seek>(r: &mut R) -> io::Result<Vec<u8>> {
    r.seek(SeekFrom::Start(0))?;
    let mut buf = Vec::new();
    r.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Append one write to a log under an exclusive lock. `build` reads what it
/// needs from the locked file and returns the bytes to write; they go out in
/// one `write_all`, then fsync.
pub fn locked_write<E: From<io::Error>>(
    path: &Path,
    build: impl FnOnce(&mut fs::File) -> Result<Vec<u8>, E>,
) -> Result<(), E> {
    let mut f = open_log(path)?;
    f.lock()?;
    let bytes = build(&mut f)?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    Ok(())
}

/// `locked_write` where `build` sees only the log's tail.
pub fn locked_append<E: From<io::Error>>(
    path: &Path,
    build: impl FnOnce(Tail) -> Result<Vec<u8>, E>,
) -> Result<(), E> {
    locked_write(path, |f| build(read_tail(f)?))
}

/// Append `v` as one canonical JSON line under an exclusive lock. A log that ends in a
/// partial line gets a LF first, so the fragment stays its own line.
pub fn append_line<T: serde::Serialize>(path: &Path, v: &T) -> io::Result<()> {
    let mut line = canonical_line(v)?;
    locked_append(path, |tail| {
        if matches!(tail, Tail::Partial(_)) {
            line.insert(0, b'\n');
        }
        Ok::<_, io::Error>(line)
    })
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
    let tmp = temp_beside(path)?;
    let out = stage(&tmp, bytes, mode).and_then(|()| fs::rename(&tmp, path));
    if out.is_err() {
        let _ = fs::remove_file(&tmp);
        return out;
    }
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::File::open(dir)?.sync_all()
}

fn stage(tmp: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let mut f = create_new(tmp, mode)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// A unique sibling of `path` for staging a write: `.<name>.<pid>.<random>.tmp`.
pub fn temp_beside(path: &Path) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_string_lossy();
    let mut rnd = [0u8; 8];
    getrandom::fill(&mut rnd).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(path.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        hex::encode(rnd)
    )))
}

/// Stream `r` into a new file at `path` with exactly `mode`, whatever the umask, then
/// publish it with `hard_link`, which fails if `path` exists. The temp file is always
/// removed.
#[cfg(unix)]
pub fn create_from(path: &Path, r: &mut impl Read, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let tmp = temp_beside(path)?;
    let out = create_new(&tmp, 0o600).and_then(|mut f| {
        io::copy(r, &mut f)?;
        f.set_permissions(fs::Permissions::from_mode(mode))?;
        f.sync_all()?;
        fs::hard_link(&tmp, path)
    });
    let _ = fs::remove_file(&tmp);
    out
}

/// Set `dir` and every folder below it to mode 0o555, so no entry can be added, removed or
/// renamed in them. Files keep their modes; links are not followed.
#[cfg(unix)]
pub fn seal(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for e in fs::read_dir(dir)? {
        let e = e?;
        if e.file_type()?.is_dir() {
            seal(&e.path())?;
        }
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o555))
}

/// Create `path` for writing; fails if it exists. `mode` applies on unix only.
pub fn create_new(path: &Path, mode: u32) -> io::Result<fs::File> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, mode);
    #[cfg(not(unix))]
    let _ = mode;
    opts.open(path)
}
