//! Signed, hash-chained record log: one JSON line per record.
use crate::canon::{canonical_of, sha256_hex};
use crate::files::{Tail, locked_append};
use crate::home::Home;
use crate::keys::{sign_hex, verify_hex};
use crate::time::now_rfc3339;
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

const GENESIS_PREV: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub seq: u64,
    pub prev: String,
    pub study: String,
    pub row: Option<u64>,
    pub kind: String,
    pub body: serde_json::Value,
    pub ts: String,
}

/// What the caller supplies; `append` fills `seq`, `prev` and `ts`.
#[derive(Debug, Clone)]
pub struct RecordDraft {
    pub study: String,
    pub row: Option<u64>,
    pub kind: String,
    pub body: serde_json::Value,
}

/// One stored line.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    record: Record,
    hash: String,
    sig: String,
}

#[derive(Debug)]
pub enum RecordError {
    /// The last line of the log is not a whole stored record.
    CorruptTail,
    Io(io::Error),
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordError::CorruptTail => write!(f, "last line of the log is not a whole record"),
            RecordError::Io(e) => write!(f, "I/O failed: {e}"),
        }
    }
}

impl std::error::Error for RecordError {}

impl From<io::Error> for RecordError {
    fn from(e: io::Error) -> Self {
        RecordError::Io(e)
    }
}

#[derive(Debug)]
pub enum VerifyError {
    /// Line `line` (1-based) is truncated or not a stored record.
    Parse {
        line: usize,
    },
    BadHash {
        seq: u64,
    },
    BadSig {
        seq: u64,
    },
    BrokenChain {
        seq: u64,
    },
    Io(io::Error),
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::Parse { line } => write!(f, "line {line} is not a whole record"),
            VerifyError::BadHash { seq } => write!(f, "record {seq}: hash does not match"),
            VerifyError::BadSig { seq } => write!(f, "record {seq}: signature does not match"),
            VerifyError::BrokenChain { seq } => write!(f, "record {seq}: chain is broken"),
            VerifyError::Io(e) => write!(f, "I/O failed: {e}"),
        }
    }
}

impl std::error::Error for VerifyError {}

impl From<io::Error> for VerifyError {
    fn from(e: io::Error) -> Self {
        VerifyError::Io(e)
    }
}

fn parse_line(line: &[u8]) -> Option<Stored> {
    serde_json::from_slice(line.strip_suffix(b"\n")?).ok()
}

/// Append one record under an exclusive lock. Returns the stored record.
pub fn append(path: &Path, key: &SigningKey, d: RecordDraft) -> Result<Record, RecordError> {
    let mut out = None;
    locked_append(path, |tail| {
        let (seq, prev) = match tail {
            Tail::Empty => (0, GENESIS_PREV.to_string()),
            Tail::Line(l) => {
                let last = parse_line(&l).ok_or(RecordError::CorruptTail)?;
                (last.record.seq + 1, last.hash)
            }
            Tail::Partial(_) => return Err(RecordError::CorruptTail),
        };
        let record = Record {
            seq,
            prev,
            study: d.study,
            row: d.row,
            kind: d.kind,
            body: d.body,
            ts: now_rfc3339(),
        };
        let bytes = canonical_of(&record)?;
        let stored = Stored {
            hash: sha256_hex(&bytes),
            sig: sign_hex(key, &bytes),
            record,
        };
        let mut line = canonical_of(&stored)?;
        line.push(b'\n');
        out = Some(stored.record);
        Ok(line)
    })?;
    out.ok_or_else(|| io::Error::other("no record built").into())
}

/// Check every line in order. Returns the number of records.
pub fn verify(path: &Path, key: &VerifyingKey) -> Result<u64, VerifyError> {
    read_all(path, key).map(|r| r.len() as u64)
}

/// Check every line in order and return the records. A missing file holds none. The file is
/// read once, so each record returned is a line that was checked.
pub fn read_all(path: &Path, key: &VerifyingKey) -> Result<Vec<Record>, VerifyError> {
    let buf = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut prev = GENESIS_PREV.to_string();
    let mut out = Vec::new();
    for (i, raw) in buf.split_inclusive(|&b| b == b'\n').enumerate() {
        let parse = VerifyError::Parse { line: i + 1 };
        let s = parse_line(raw).ok_or(parse)?;
        let seq = s.record.seq;
        let bytes = canonical_of(&s.record)?;
        if sha256_hex(&bytes) != s.hash {
            return Err(VerifyError::BadHash { seq });
        }
        if !verify_hex(key, &bytes, &s.sig) {
            return Err(VerifyError::BadSig { seq });
        }
        if seq != out.len() as u64 || s.record.prev != prev {
            return Err(VerifyError::BrokenChain { seq });
        }
        prev = s.hash;
        out.push(s.record);
    }
    Ok(out)
}

/// Hex SHA-256 of the record's canonical bytes: the `hash` that `append` stores.
pub fn hash(r: &Record) -> String {
    // A record is plain data, so canonical serialization cannot fail.
    sha256_hex(&canonical_of(r).unwrap_or_default())
}

/// The record with this hash, from any study log in the box, read in sorted folder order.
/// A log that fails verification is an error, never skipped.
pub fn find(home: &Home, key: &VerifyingKey, hash: &str) -> Result<Option<Record>, VerifyError> {
    let mut dirs = Vec::new();
    match std::fs::read_dir(home.root.join("studies")) {
        Ok(rd) => {
            for e in rd {
                dirs.push(e?.path());
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    dirs.sort();
    // Every log is verified, even after a match: a box with a damaged log answers nothing.
    let mut found = None;
    for d in dirs {
        for r in read_all(&d.join("records.jsonl"), key)? {
            if found.is_none() && self::hash(&r) == hash {
                found = Some(r);
            }
        }
    }
    Ok(found)
}
