//! The plan log on disk: `leaves.jsonl`, `heads.jsonl` and a `lock` file.
use super::merkle;
use crate::canon::{canonical_of, to_canonical};
use crate::files::{locked_append, open_log};
use crate::home::{HomeError, make_private_dir};
use crate::keys::{sign_hex, verify_hex};
use crate::prior::{PriorError, PriorUse};
use crate::time::now_rfc3339;
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io;
use std::path::{Path, PathBuf};

/// No `deny_unknown_fields`: later versions may add leaf fields, and
/// older binaries must still read the log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Leaf {
    /// `plan` or `legacy`.
    pub kind: String,
    pub commitment: Option<String>,
    pub family: Option<String>,
    pub mode: Option<String>,
    /// `(exact sha256, normalized sha256)` per data file.
    pub data_hashes: Vec<(String, String)>,
    pub label: String,
    pub ts: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeHead {
    pub size: u64,
    pub root: String,
    pub ts: String,
    pub sig: String,
}

#[derive(Debug)]
pub enum LogError {
    /// A log file does not end with a line feed.
    CorruptTail,
    /// Head `index` (0-based) is not valid.
    Head {
        index: usize,
        reason: &'static str,
    },
    /// Leaf line `index` (0-based) is not a leaf.
    Leaf {
        index: usize,
    },
    Dir(HomeError),
    Io(io::Error),
}

impl std::fmt::Display for LogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogError::CorruptTail => write!(f, "a plan log file does not end with a whole line"),
            LogError::Head { index, reason } => write!(f, "head {index}: {reason}"),
            LogError::Leaf { index } => write!(f, "leaf {index} is not a leaf"),
            LogError::Dir(e) => e.fmt(f),
            LogError::Io(e) => write!(f, "I/O failed: {e}"),
        }
    }
}

impl std::error::Error for LogError {}

impl From<io::Error> for LogError {
    fn from(e: io::Error) -> Self {
        LogError::Io(e)
    }
}

impl From<HomeError> for LogError {
    fn from(e: HomeError) -> Self {
        LogError::Dir(e)
    }
}

/// What `verify` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verified {
    /// Leaves covered by the last signed head.
    pub signed: u64,
    /// Lines in `leaves.jsonl`. More than `signed` means unsigned leaves
    /// at the end. A crash between the leaf write and the head write is
    /// the only legitimate cause; the next append signs them.
    pub leaves: u64,
}

pub struct PlanLog {
    dir: PathBuf,
}

/// Canonical JSON of `{size, root, ts}`: the bytes a head signs.
fn head_bytes(size: u64, root: &str, ts: &str) -> io::Result<Vec<u8>> {
    to_canonical(&json!({ "size": size, "root": root, "ts": ts })).map_err(io::Error::other)
}

/// The whole lines of a log file, without their line feeds.
fn read_lines(path: &Path) -> Result<Vec<Vec<u8>>, LogError> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            return Err(io::Error::other("log path is a symbolic link").into());
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    }
    let buf = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    buf.split_inclusive(|&b| b == b'\n')
        .map(|l| {
            l.strip_suffix(b"\n")
                .map(<[u8]>::to_vec)
                .ok_or(LogError::CorruptTail)
        })
        .collect()
}

fn line(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.push(b'\n');
    bytes
}

impl PlanLog {
    /// Open the log in `dir`, creating `dir` with mode 0700 if absent.
    pub fn open(dir: &Path) -> Result<PlanLog, LogError> {
        make_private_dir(dir, true, true)?;
        Ok(PlanLog {
            dir: dir.to_path_buf(),
        })
    }

    fn leaves_path(&self) -> PathBuf {
        self.dir.join("leaves.jsonl")
    }

    fn heads_path(&self) -> PathBuf {
        self.dir.join("heads.jsonl")
    }

    /// Hold the shared lock while reading. Writers wait for it.
    fn read_lock(&self) -> io::Result<std::fs::File> {
        let f = open_log(&self.dir.join("lock"))?;
        f.lock_shared()?;
        Ok(f)
    }

    pub fn heads(&self) -> Result<Vec<TreeHead>, LogError> {
        let _lock = self.read_lock()?;
        self.read_heads()
    }

    fn read_heads(&self) -> Result<Vec<TreeHead>, LogError> {
        read_lines(&self.heads_path())?
            .iter()
            .enumerate()
            .map(|(index, l)| {
                serde_json::from_slice(l).map_err(|_| LogError::Head {
                    index,
                    reason: "not a tree head",
                })
            })
            .collect()
    }

    /// Fail when `leaves.jsonl` or `heads.jsonl` ends in a partial line, the faults `append`
    /// refuses. Takes the shared lock, so call it only when this process holds no lock on the log.
    pub fn check_tails(&self) -> Result<(), LogError> {
        let _lock = self.read_lock()?;
        read_lines(&self.leaves_path())?;
        read_lines(&self.heads_path())?;
        Ok(())
    }

    /// Add a leaf and a signed head for the new tree. One exclusive lock
    /// on `dir/lock` covers the whole operation. Returns the leaf index.
    pub fn append(&self, leaf: Leaf, key: &SigningKey) -> Result<(u64, TreeHead), LogError> {
        let lock = open_log(&self.dir.join("lock"))?;
        lock.lock()?;
        let data = canonical_of(&leaf)?;
        // Both tails are checked before anything is written.
        let mut leaves = read_lines(&self.leaves_path())?;
        read_lines(&self.heads_path())?;
        let index = leaves.len() as u64;
        leaves.push(data.clone());
        let root = hex::encode(merkle::root(&leaves));
        let ts = now_rfc3339();
        let sig = sign_hex(key, &head_bytes(leaves.len() as u64, &root, &ts)?);
        let head = TreeHead {
            size: leaves.len() as u64,
            root,
            ts,
            sig,
        };
        locked_append(&self.leaves_path(), |_| Ok::<_, LogError>(line(data)))?;
        locked_append(&self.heads_path(), |_| {
            Ok::<_, LogError>(line(canonical_of(&head)?))
        })?;
        Ok((index, head))
    }

    /// Recompute each head's root from the leaves and check its signature.
    /// Head sizes must grow.
    pub fn verify(&self, key: &VerifyingKey) -> Result<Verified, LogError> {
        let _lock = self.read_lock()?;
        let leaves = read_lines(&self.leaves_path())?;
        let mut last = 0u64;
        for (index, head) in self.read_heads()?.iter().enumerate() {
            let bad = |reason| LogError::Head { index, reason };
            if head.size <= last {
                return Err(bad("size does not grow"));
            }
            let size = usize::try_from(head.size).map_err(|_| bad("size exceeds the leaves"))?;
            if size > leaves.len() {
                return Err(bad("size exceeds the leaves"));
            }
            if head.root != hex::encode(merkle::root(&leaves[..size])) {
                return Err(bad("root does not match the leaves"));
            }
            let bytes = head_bytes(head.size, &head.root, &head.ts)?;
            if !verify_hex(key, &bytes, &head.sig) {
                return Err(bad("signature does not match"));
            }
            last = head.size;
        }
        Ok(Verified {
            signed: last,
            leaves: leaves.len() as u64,
        })
    }

    /// Every leaf that holds this data, oldest first, with its index.
    pub fn uses(&self, exact: &str, normalized: &str) -> Result<Vec<(usize, Leaf)>, LogError> {
        let _lock = self.read_lock()?;
        let mut hits = Vec::new();
        for (index, l) in read_lines(&self.leaves_path())?.iter().enumerate() {
            let leaf: Leaf = serde_json::from_slice(l).map_err(|_| LogError::Leaf { index })?;
            let hit = leaf
                .data_hashes
                .iter()
                .any(|(e, n)| e == exact || (!normalized.is_empty() && n == normalized));
            if hit {
                hits.push((index, leaf));
            }
        }
        Ok(hits)
    }

    /// The leaf at `index`, or `None` past the end.
    pub fn leaf(&self, index: u64) -> Result<Option<Leaf>, LogError> {
        let _lock = self.read_lock()?;
        let lines = read_lines(&self.leaves_path())?;
        let Some(l) = usize::try_from(index).ok().and_then(|i| lines.get(i)) else {
            return Ok(None);
        };
        let index = index as usize;
        serde_json::from_slice(l)
            .map(Some)
            .map_err(|_| LogError::Leaf { index })
    }

    /// Add a `legacy` leaf that records data files seen under `label`.
    pub fn append_legacy(
        &self,
        label: &str,
        data_hashes: Vec<(String, String)>,
        key: &SigningKey,
    ) -> Result<(u64, TreeHead), LogError> {
        self.append(
            Leaf {
                kind: "legacy".into(),
                commitment: None,
                family: None,
                mode: None,
                data_hashes,
                label: label.into(),
                ts: now_rfc3339(),
            },
            key,
        )
    }
}

impl PriorUse for PlanLog {
    fn used(&self, exact: &str, normalized: &str) -> Result<Option<String>, PriorError> {
        self.uses(exact, normalized)
            .map(|hits| {
                hits.first()
                    .map(|(i, leaf)| format!("leaf {i} ({})", leaf.kind))
            })
            .map_err(|e| PriorError(e.to_string()))
    }
}
