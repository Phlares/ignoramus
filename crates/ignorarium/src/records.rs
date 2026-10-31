//! Signed, hash-chained record log: one JSON line per record.
use crate::canon::{canonical_line, canonical_of, sha256_hex};
use crate::files::{Tail, locked_append, locked_write, read_whole};
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

impl RecordDraft {
    /// A record of `study` that belongs to no row.
    pub fn new(study: &str, kind: &str, body: serde_json::Value) -> RecordDraft {
        RecordDraft {
            study: study.into(),
            row: None,
            kind: kind.into(),
            body,
        }
    }
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
    /// A record already in the log fails verification.
    Log(VerifyError),
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordError::CorruptTail => write!(f, "last line of the log is not a whole record"),
            RecordError::Io(e) => write!(f, "I/O failed: {e}"),
            RecordError::Log(e) => write!(f, "the log fails verification: {e}"),
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
    /// The failure is in the log of this study.
    In {
        study: String,
        cause: Box<VerifyError>,
    },
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::Parse { line } => write!(f, "line {line} is not a whole record"),
            VerifyError::BadHash { seq } => write!(f, "record {seq}: hash does not match"),
            VerifyError::BadSig { seq } => write!(f, "record {seq}: signature does not match"),
            VerifyError::BrokenChain { seq } => write!(f, "record {seq}: chain is broken"),
            VerifyError::Io(e) => write!(f, "I/O failed: {e}"),
            VerifyError::In { study, cause } => write!(f, "study {study}: {cause}"),
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

/// The draft as record `seq`, chained after the record whose hash is `prev`, and signed.
fn seal(seq: u64, prev: String, d: RecordDraft, key: &SigningKey) -> io::Result<Stored> {
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
    Ok(Stored {
        hash: sha256_hex(&bytes),
        sig: sign_hex(key, &bytes),
        record,
    })
}

/// Append one record under an exclusive lock, chained to the last line alone. Returns the
/// stored record.
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
        let stored = seal(seq, prev, d, key)?;
        let line = canonical_line(&stored)?;
        out = Some(stored.record);
        Ok(line)
    })?;
    out.ok_or_else(|| io::Error::other("no record built").into())
}

/// `make`'s error, or an I/O fault as a `RecordError`.
struct Failed<E>(E);

impl<E: From<RecordError>> From<io::Error> for Failed<E> {
    fn from(e: io::Error) -> Self {
        Failed(E::from(RecordError::Io(e)))
    }
}

/// Under the log's exclusive lock, read and verify every record, then append what `make`
/// returns for them, chained after the last, in one write. A log that fails verification is
/// refused before `make` runs; when `make` fails, nothing is written. Returns the records
/// appended.
pub fn append_with<E: From<RecordError>>(
    path: &Path,
    key: &SigningKey,
    make: impl FnOnce(&[Record]) -> Result<Vec<RecordDraft>, E>,
) -> Result<Vec<Record>, E> {
    let mut out = Vec::new();
    locked_write(path, |f| -> Result<Vec<u8>, Failed<E>> {
        let buf = read_whole(f)?;
        let (all, mut prev) = verified(&buf, &key.verifying_key())
            .map_err(|e| Failed(E::from(RecordError::Log(e))))?;
        let mut lines = Vec::new();
        for d in make(&all).map_err(Failed)? {
            let stored = seal((all.len() + out.len()) as u64, prev, d, key)?;
            lines.extend(canonical_line(&stored)?);
            prev = stored.hash;
            out.push(stored.record);
        }
        Ok(lines)
    })
    .map_err(|Failed(e)| e)?;
    Ok(out)
}

/// Check every line in order. Returns the number of records.
pub fn verify(path: &Path, key: &VerifyingKey) -> Result<u64, VerifyError> {
    read_all(path, key).map(|r| r.len() as u64)
}

/// Check every line in order and return the records. A missing file holds none. The file is
/// read once, so each record returned is a line that was checked.
pub fn read_all(path: &Path, key: &VerifyingKey) -> Result<Vec<Record>, VerifyError> {
    match std::fs::read(path) {
        Ok(buf) => verified(&buf, key).map(|(all, _)| all),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

/// The first record of the log at `path`, checked as `read_all` checks it, without reading
/// the rest of the file; `None` for a missing or empty log.
pub fn first(path: &Path, key: &VerifyingKey) -> Result<Option<Record>, VerifyError> {
    use std::io::BufRead;
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut line = Vec::new();
    io::BufReader::new(file).read_until(b'\n', &mut line)?;
    Ok(verified(&line, key)?.0.into_iter().next())
}

/// The records `buf` holds, each line checked in order, and the hash of the last one (the
/// genesis value when there is none).
fn verified(buf: &[u8], key: &VerifyingKey) -> Result<(Vec<Record>, String), VerifyError> {
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
    Ok((out, prev))
}

/// Hex SHA-256 of the record's canonical bytes: the `hash` that `append` stores.
pub fn hash(r: &Record) -> String {
    let bytes =
        canonical_of(r).expect("a record is plain data, so canonical serialization cannot fail");
    sha256_hex(&bytes)
}

/// The study folders of the box that hold a record log, as (study id, log path), sorted by id.
/// Entries that are not folders are skipped.
pub fn logs(home: &Home) -> io::Result<Vec<(String, std::path::PathBuf)>> {
    let mut out = Vec::new();
    match std::fs::read_dir(home.root.join("studies")) {
        Ok(rd) => {
            for e in rd {
                let e = e?;
                let log = e.path().join("records.jsonl");
                if e.file_type()?.is_dir() && log.exists() {
                    out.push((e.file_name().to_string_lossy().into_owned(), log));
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    out.sort();
    Ok(out)
}

/// The record with this hash, from any study log in the box, read in sorted folder order.
/// A log that fails verification is an error that names its study, never skipped.
pub fn find(home: &Home, key: &VerifyingKey, hash: &str) -> Result<Option<Record>, VerifyError> {
    // Every log is verified, even after a match: a box with a damaged log answers nothing.
    let mut found = None;
    for (study, log) in logs(home)? {
        let read = read_all(&log, key).map_err(|cause| VerifyError::In {
            study,
            cause: Box::new(cause),
        })?;
        for r in read {
            if found.is_none() && self::hash(&r) == hash {
                found = Some(r);
            }
        }
    }
    Ok(found)
}
