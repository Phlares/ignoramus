//! The plan log on disk: `leaves.jsonl`, `heads.jsonl` and a `lock` file.
use super::merkle;
use crate::canon::to_canonical;
use crate::files::{Tail, locked_append, open_log};
use crate::home::{HomeError, make_private_dir};
use crate::prior::PriorUse;
use crate::time::now_rfc3339;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    Dir(HomeError),
    Io(io::Error),
}

impl std::fmt::Display for LogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogError::CorruptTail => write!(f, "a plan log file does not end with a whole line"),
            LogError::Head { index, reason } => write!(f, "head {index}: {reason}"),
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

pub struct PlanLog {
    dir: PathBuf,
}

/// Canonical JSON of `{size, root, ts}`: the bytes a head signs.
fn head_bytes(size: u64, root: &str, ts: &str) -> io::Result<Vec<u8>> {
    to_canonical(&json!({ "size": size, "root": root, "ts": ts })).map_err(io::Error::other)
}

/// The whole lines of a log file, without their line feeds.
fn read_lines(path: &Path) -> Result<Vec<Vec<u8>>, LogError> {
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

/// Refuse to append after a partial line.
fn whole_tail(tail: Tail) -> Result<(), LogError> {
    match tail {
        Tail::Partial(_) => Err(LogError::CorruptTail),
        _ => Ok(()),
    }
}

fn line(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.push(b'\n');
    bytes
}

impl PlanLog {
    /// Open the log in `dir`, creating `dir` with mode 0700 if absent.
    pub fn open(dir: &Path) -> Result<PlanLog, LogError> {
        make_private_dir(dir, true)?;
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

    pub fn heads(&self) -> Result<Vec<TreeHead>, LogError> {
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

    /// Add a leaf and a signed head for the new tree. One exclusive lock
    /// on `dir/lock` covers the whole operation. Returns the leaf index.
    pub fn append(&self, leaf: Leaf, key: &SigningKey) -> Result<(u64, TreeHead), LogError> {
        let lock = open_log(&self.dir.join("lock"))?;
        lock.lock()?;
        let data = to_canonical(&serde_json::to_value(&leaf).map_err(io::Error::other)?)
            .map_err(io::Error::other)?;
        let mut leaves = read_lines(&self.leaves_path())?;
        let index = leaves.len() as u64;
        leaves.push(data.clone());
        let root = hex::encode(merkle::root(&leaves));
        let ts = now_rfc3339();
        let sig = hex::encode(
            key.sign(&head_bytes(leaves.len() as u64, &root, &ts)?)
                .to_bytes(),
        );
        let head = TreeHead {
            size: leaves.len() as u64,
            root,
            ts,
            sig,
        };
        locked_append(&self.leaves_path(), |tail| {
            whole_tail(tail)?;
            Ok::<_, LogError>(line(data))
        })?;
        locked_append(&self.heads_path(), |tail| {
            whole_tail(tail)?;
            let v = serde_json::to_value(&head).map_err(io::Error::other)?;
            Ok::<_, LogError>(line(to_canonical(&v).map_err(io::Error::other)?))
        })?;
        Ok((index, head))
    }

    /// Recompute each head's root from the leaves and check its signature.
    /// Head sizes must grow. Returns the number of leaves.
    pub fn verify(&self, key: &VerifyingKey) -> Result<u64, LogError> {
        let leaves = read_lines(&self.leaves_path())?;
        let mut last = 0u64;
        for (index, head) in self.heads()?.iter().enumerate() {
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
            hex::decode(&head.sig)
                .ok()
                .and_then(|b| Signature::from_slice(&b).ok())
                .filter(|s| key.verify(&bytes, s).is_ok())
                .ok_or_else(|| bad("signature does not match"))?;
            last = head.size;
        }
        Ok(leaves.len() as u64)
    }
}

impl PriorUse for PlanLog {
    fn used(&self, exact_sha256: &str, normalized_sha256: &str) -> Option<String> {
        let lines = read_lines(&self.leaves_path()).ok()?;
        lines.iter().enumerate().find_map(|(i, l)| {
            let leaf: Leaf = serde_json::from_slice(l).ok()?;
            let hit = leaf.data_hashes.iter().any(|(e, n)| {
                e == exact_sha256 || (!normalized_sha256.is_empty() && n == normalized_sha256)
            });
            hit.then(|| format!("leaf {i} ({})", leaf.kind))
        })
    }
}
