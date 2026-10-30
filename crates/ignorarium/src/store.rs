//! Content-addressed store: `store/<sha256>` holds the bytes with that hash.
use crate::canon::sha256_hex;
use crate::files::{self, WriteOnceError};
use crate::home::{HomeError, make_private_dir};
use crate::prior::file_hashes_within;
use crate::records::{self, RecordDraft};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const MODE: u32 = 0o444;

pub use crate::prior::NORMALIZE_LIMIT;

/// What the store knows about one blob besides its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub size: u64,
    /// Empty when the blob has no normalized hash.
    pub normalized_sha256: String,
}

#[derive(Debug)]
pub enum StoreError {
    /// Not 64 lowercase hex characters.
    BadDigest,
    /// The store folder or a blob path is a symbolic link.
    Link,
    /// An import source is not a regular file.
    NotRegular,
    Io(io::Error),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::BadDigest => write!(f, "digest is not 64 lowercase hex characters"),
            StoreError::Link => write!(f, "store path is a symbolic link"),
            StoreError::NotRegular => write!(f, "not a regular file"),
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
                let path = self.root.join(&digest);
                if fs::symlink_metadata(&path)?.file_type().is_symlink() {
                    return Err(StoreError::Link);
                }
                // The blob was there before: its bytes must still be the digest's.
                let mut hasher = Sha256::new();
                io::copy(&mut fs::File::open(&path)?, &mut hasher)?;
                if hex::encode(hasher.finalize()) != digest {
                    return Err(io::Error::other("stored blob differs").into());
                }
            }
            Err(e) => return Err(e.into()),
        }
        Ok(digest)
    }

    /// Add the regular file `file` to the store: stream it under its digest and write its
    /// `Meta` once. A link or a file of another kind is refused, and so is a stored blob
    /// that does not hash to its digest.
    pub fn import(&self, file: &Path) -> Result<(String, Meta), StoreError> {
        self.import_within(file, NORMALIZE_LIMIT)
    }

    /// `import` with another limit for the normalized hash.
    pub fn import_within(&self, file: &Path, limit: u64) -> Result<(String, Meta), StoreError> {
        let (digest, meta) = self.stream_in(file, limit)?;
        self.write_meta(&digest, &meta)?;
        Ok((digest, meta))
    }

    /// The blob of `file` in the store, and its `Meta`, not yet written.
    fn stream_in(&self, file: &Path, limit: u64) -> Result<(String, Meta), StoreError> {
        let kind = fs::symlink_metadata(file)?.file_type();
        if kind.is_symlink() {
            return Err(StoreError::Link);
        }
        if !kind.is_file() {
            return Err(StoreError::NotRegular);
        }
        let mut f = fs::File::open(file)?;
        if !f.metadata()?.is_file() {
            return Err(StoreError::NotRegular);
        }
        // One read: the stream feeds the blob, the size and the normalized hash.
        let mut seen = Seen {
            inner: &mut f,
            size: 0,
            head: Vec::new(),
            limit,
        };
        let digest = self.put_reader(&mut seen)?;
        let meta = Meta {
            size: seen.size,
            normalized_sha256: file_hashes_within(&seen.head, limit).1,
        };
        Ok((digest, meta))
    }

    fn write_meta(&self, digest: &str, meta: &Meta) -> Result<(), StoreError> {
        let dir = self.root.join("meta");
        make_private_dir(&dir, false, true)?;
        let bytes = serde_json::to_vec(meta).map_err(io::Error::other)?;
        files::write_once(&dir.join(format!("{digest}.json")), &bytes, MODE)?;
        Ok(())
    }

    /// `import`, signing one `import` record (see `sign_import`) when the store did not
    /// know the blob as an input yet, so every stored input has the record of how it came
    /// in, and a resume that imports the same file again adds none. The record is signed
    /// before the meta is written: a crash between the two signs it again, never not at all.
    pub fn import_signed(
        &self,
        file: &Path,
        key: &SigningKey,
        source: &str,
    ) -> Result<(String, Meta), StoreError> {
        let (digest, meta) = self.stream_in(file, NORMALIZE_LIMIT)?;
        if self.meta(&digest)?.is_none() {
            self.sign_import(key, &digest, &meta, source, file)?;
        }
        self.write_meta(&digest, &meta)?;
        Ok((digest, meta))
    }

    /// One signed `import` record in `imports.jsonl` that names `source` and the file's
    /// name, never its path.
    pub fn sign_import(
        &self,
        key: &SigningKey,
        digest: &str,
        meta: &Meta,
        source: &str,
        file: &Path,
    ) -> Result<(), StoreError> {
        let body = json!({
            "digest": digest, "size": meta.size,
            "normalized_sha256": meta.normalized_sha256,
            "source": source, "file": record_name(file),
        });
        let draft = RecordDraft {
            study: "store".into(),
            row: None,
            kind: "import".into(),
            body,
        };
        records::append(&self.root.join("imports.jsonl"), key, draft)
            .map_err(|e| io::Error::other(format!("imports.jsonl: {e}")))?;
        Ok(())
    }

    /// The `Meta` of an imported blob, or `None` when the store has none for `digest`.
    pub fn meta(&self, digest: &str) -> Result<Option<Meta>, StoreError> {
        self.path(digest)?;
        let path = self.root.join("meta").join(format!("{digest}.json"));
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            )),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
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

/// The name an import record keeps for `file`: its last component.
pub fn record_name(file: &Path) -> String {
    file.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
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

/// A reader that counts what passes and keeps the first `limit + 1` bytes: enough to
/// normalize a file within the limit, and to see that a larger one is over it.
struct Seen<'a, R> {
    inner: &'a mut R,
    size: u64,
    head: Vec<u8>,
    limit: u64,
}

impl<R: Read> Read for Seen<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.size += n as u64;
        let room = (self.limit + 1).saturating_sub(self.head.len() as u64) as usize;
        self.head.extend_from_slice(&buf[..n.min(room)]);
        Ok(n)
    }
}
