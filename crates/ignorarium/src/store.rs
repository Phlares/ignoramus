//! Content-addressed store: `store/<sha256>` holds the bytes with that hash.
use crate::canon::sha256_hex;
use crate::files::{self, WriteOnceError};
use crate::home::{HomeError, make_private_dir};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const MODE: u32 = 0o444;

#[derive(Debug)]
pub enum StoreError {
    /// Not 64 lowercase hex characters.
    BadDigest,
    /// The store folder or a blob path is a symbolic link.
    Link,
    Io(io::Error),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::BadDigest => write!(f, "digest is not 64 lowercase hex characters"),
            StoreError::Link => write!(f, "store path is a symbolic link"),
            StoreError::Io(e) => write!(f, "I/O failed: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        StoreError::Io(e)
    }
}

impl From<HomeError> for StoreError {
    fn from(e: HomeError) -> Self {
        match e {
            HomeError::Link(_) => StoreError::Link,
            HomeError::Io(e) => StoreError::Io(e),
            other => StoreError::Io(io::Error::other(other.to_string())),
        }
    }
}

impl From<WriteOnceError> for StoreError {
    fn from(e: WriteOnceError) -> Self {
        match e {
            WriteOnceError::Link => StoreError::Link,
            WriteOnceError::Io(e) => StoreError::Io(e),
            // A blob under its own hash differs only if the store is corrupt.
            WriteOnceError::Differs => StoreError::Io(io::Error::other("stored blob differs")),
        }
    }
}

pub struct Store {
    pub root: PathBuf,
}

impl Store {
    /// Open `home/store/`, creating it with mode 0700. Refuses a symbolic link.
    pub fn open(home: &Path) -> Result<Store, StoreError> {
        let root = home.join("store");
        make_private_dir(&root, true, true)?;
        Ok(Store { root })
    }

    /// Write `bytes` once under their SHA-256 and return the hex digest.
    pub fn put(&self, bytes: &[u8]) -> Result<String, StoreError> {
        let digest = sha256_hex(bytes);
        files::write_once(&self.root.join(&digest), bytes, MODE)?;
        Ok(digest)
    }

    /// Stream `r` to a temp file while hashing, then publish under the digest.
    pub fn put_reader(&self, r: &mut impl Read) -> Result<String, StoreError> {
        let tmp = files::temp_beside(&self.root.join("blob"))?;
        let out = self.stream_and_publish(&tmp, r);
        let _ = fs::remove_file(&tmp);
        out
    }

    fn stream_and_publish(&self, tmp: &Path, r: &mut impl Read) -> Result<String, StoreError> {
        let mut tee = Tee {
            file: files::create_new(tmp, MODE)?,
            hasher: Sha256::new(),
        };
        io::copy(r, &mut tee)?;
        tee.file.sync_all()?;
        let digest = hex::encode(tee.hasher.finalize());
        match fs::hard_link(tmp, self.root.join(&digest)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                // `hard_link` also reports AlreadyExists for a symlink at the path.
                if fs::symlink_metadata(self.root.join(&digest))?
                    .file_type()
                    .is_symlink()
                {
                    return Err(StoreError::Link);
                }
            }
            Err(e) => return Err(e.into()),
        }
        Ok(digest)
    }

    /// The blob path for `digest`. Refuses anything but 64 lowercase hex characters.
    pub fn path(&self, digest: &str) -> Result<PathBuf, StoreError> {
        if digest.len() == 64
            && digest
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            Ok(self.root.join(digest))
        } else {
            Err(StoreError::BadDigest)
        }
    }

    /// True when a blob with this digest exists. A bad digest is never present.
    pub fn has(&self, digest: &str) -> bool {
        self.path(digest).is_ok_and(|p| p.is_file())
    }
}

/// A writer that hashes what it writes.
struct Tee {
    file: fs::File,
    hasher: Sha256,
}

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.file.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
