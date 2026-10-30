//! The input view: `<study>/in` holds one hard link per input to its blob in the store, so a
//! row sees the declared inputs and nothing else. A link shares the blob's bytes and its
//! read-only mode; the view's folders are read-only too.
use crate::backend::output::file_digest;
use crate::files;
use crate::rules::input_path;
use crate::store::Store;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// The view folder's name inside the study folder.
pub const IN: &str = "in";

#[derive(Debug)]
pub enum ViewError {
    /// The store has no blob for this input's digest.
    Missing(String),
    /// The input's file no longer hashes to its digest.
    Changed(String),
    /// The input path could leave the view.
    Unsafe(String),
    Io(io::Error),
}

impl fmt::Display for ViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ViewError::Missing(p) => write!(f, "the store has no blob for input {p}"),
            ViewError::Changed(p) => write!(f, "input {p} no longer hashes to its digest"),
            ViewError::Unsafe(p) => write!(f, "input path {p} could leave the view"),
            ViewError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ViewError {}

impl From<io::Error> for ViewError {
    fn from(e: io::Error) -> Self {
        ViewError::Io(e)
    }
}

/// What a stat shows of one file: enough to see that it changed without reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: (i64, i64),
}

/// One entry below a view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Folder,
    File(Stamp),
    /// A link, a FIFO, or a name that is not UTF-8.
    Other,
}

/// Every entry of a view by its path below the view; `""` is the view folder itself.
pub type Scan = BTreeMap<String, Entry>;

/// The path of an input below the view: its segments joined by `/`, empty ones dropped.
pub fn key(path: &str) -> String {
    path.split('/')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// Make `study_dir/in` hold exactly one hard link per `(path, digest)` to its blob and
/// return its absolute path. A view whose entries are already exactly those links (same
/// device and inode) is kept; any other is renamed to the first free `in.old-<k>` and built
/// again. Nothing is deleted.
pub fn build(
    store: &Store,
    study_dir: &Path,
    inputs: &[(String, String)],
) -> Result<PathBuf, ViewError> {
    let dir = std::path::absolute(study_dir.join(IN))?;
    let want = expected(store, inputs)?;
    let kept = match dir.symlink_metadata() {
        Ok(_) => scan(&dir).is_ok_and(|s| s == want),
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.into()),
    };
    if !kept {
        if dir.symlink_metadata().is_ok() {
            let aside = (0u32..)
                .map(|k| study_dir.join(format!("{IN}.old-{k}")))
                .find(|p| p.symlink_metadata().is_err())
                .ok_or_else(|| io::Error::other("no free name for the old input view"))?;
            fs::rename(&dir, aside)?;
        }
        fs::create_dir(&dir)?;
        for (path, digest) in inputs {
            let link = input_path(&dir, path).ok_or_else(|| ViewError::Unsafe(path.clone()))?;
            if let Some(parent) = link.parent() {
                fs::create_dir_all(parent)?;
            }
            let blob = store
                .path(digest)
                .map_err(|_| ViewError::Missing(path.clone()))?;
            fs::hard_link(blob, &link)?;
        }
    }
    files::seal(&dir)?;
    Ok(dir)
}

/// The scan a view of exactly these blobs gives.
fn expected(store: &Store, inputs: &[(String, String)]) -> Result<Scan, ViewError> {
    let mut want = Scan::from([(String::new(), Entry::Folder)]);
    for (path, digest) in inputs {
        let missing = || ViewError::Missing(path.clone());
        let blob = store.path(digest).map_err(|_| missing())?;
        let m = blob.symlink_metadata().map_err(|_| missing())?;
        if !m.is_file() {
            return Err(missing());
        }
        let k = key(path);
        let mut at = k.as_str();
        while let Some((up, _)) = at.rsplit_once('/') {
            want.insert(up.to_string(), Entry::Folder);
            at = up;
        }
        want.insert(k, entry(&m));
    }
    Ok(want)
}

/// Every entry below `dir`, without following a link.
pub fn scan(dir: &Path) -> io::Result<Scan> {
    let m = dir.symlink_metadata()?;
    let mut s = Scan::from([(String::new(), entry(&m))]);
    if m.is_dir() {
        walk(dir, "", &mut s)?;
    }
    Ok(s)
}

fn walk(dir: &Path, prefix: &str, s: &mut Scan) -> io::Result<()> {
    for e in fs::read_dir(dir)? {
        let e = e?;
        let Some(name) = e.file_name().to_str().map(String::from) else {
            // No input path holds a NUL, so this key never matches one.
            s.insert("\0".into(), Entry::Other);
            continue;
        };
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let m = e.metadata()?;
        s.insert(rel.clone(), entry(&m));
        if m.is_dir() {
            walk(&e.path(), &rel, s)?;
        }
    }
    Ok(())
}

fn entry(m: &fs::Metadata) -> Entry {
    if m.is_dir() {
        Entry::Folder
    } else if m.is_file() {
        Entry::File(Stamp {
            dev: m.dev(),
            ino: m.ino(),
            size: m.size(),
            mtime: (m.mtime(), m.mtime_nsec()),
        })
    } else {
        Entry::Other
    }
}

/// Hash every input file in the view in full and return the scan taken before the hashing,
/// the baseline a later `scan` is compared with. The first file that does not hash to its
/// digest is `Changed`.
pub fn verify(dir: &Path, inputs: &[(String, String)]) -> Result<Scan, ViewError> {
    let seen = scan(dir)?;
    for (path, digest) in inputs {
        let file = dir.join(key(path));
        if file_digest(&file)?.as_deref() != Some(digest.as_str()) {
            return Err(ViewError::Changed(path.clone()));
        }
    }
    Ok(seen)
}
