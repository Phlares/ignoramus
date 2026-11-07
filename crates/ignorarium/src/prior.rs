//! Earlier-use lookup: has a data file already been seen by an earlier study?
use crate::canon::sha256_hex;
use std::collections::HashSet;

/// The largest file that gets a normalized hash. Larger files, and files that are not
/// UTF-8 text, have none.
pub const NORMALIZE_LIMIT: u64 = 64 << 20;

/// The earlier-use log cannot be read. Callers must treat this as a refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorError(pub String);

impl std::fmt::Display for PriorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PriorError {}

pub trait PriorUse {
    /// The id of the first earlier log entry that used this data and whose leaf index
    /// `counts` accepts, if any. `normalized_sha256` is empty when the file is not UTF-8 text.
    fn used_where(
        &self,
        exact_sha256: &str,
        normalized_sha256: &str,
        counts: &dyn Fn(u64) -> bool,
    ) -> Result<Option<String>, PriorError>;

    /// `used_where`, counting every entry.
    fn used(
        &self,
        exact_sha256: &str,
        normalized_sha256: &str,
    ) -> Result<Option<String>, PriorError> {
        self.used_where(exact_sha256, normalized_sha256, &|_| true)
    }
}

/// A log with no entries.
pub struct NoPriorUse;

impl PriorUse for NoPriorUse {
    fn used_where(
        &self,
        _: &str,
        _: &str,
        _: &dyn Fn(u64) -> bool,
    ) -> Result<Option<String>, PriorError> {
        Ok(None)
    }
}

/// SHA-256 of the text, lowercased, with whitespace runs folded to one space.
pub fn normalized_sha256(text: &str) -> String {
    let folded = text
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    sha256_hex(folded.as_bytes())
}

/// Two files, by their `(exact, normalized)` hashes, hold the same data: the same exact hash,
/// or the same normalized hash (text that differs only in case and spacing). An empty
/// normalized hash (no text) matches nothing.
pub fn same_data(a: &(String, String), b: &(String, String)) -> bool {
    a.0 == b.0 || (!a.1.is_empty() && a.1 == b.1)
}

/// The data kept so far, for a walk that keeps only the first input of each data
/// (`same_data`), at a set lookup per input.
#[derive(Default)]
pub struct Seen {
    exact: HashSet<String>,
    normalized: HashSet<String>,
}

impl Seen {
    /// Keeps `h` and returns `true` when no data kept so far is the same; else `false`, and `h`
    /// stays out, so a later input is compared with the kept data alone.
    pub fn insert(&mut self, h: &(String, String)) -> bool {
        let empty = h.1.is_empty();
        if self.exact.contains(&h.0) || (!empty && self.normalized.contains(&h.1)) {
            return false;
        }
        self.exact.insert(h.0.clone());
        if !empty {
            self.normalized.insert(h.1.clone());
        }
        true
    }
}

/// `(exact, normalized)` SHA-256 of a file. The normalized hash is empty
/// when the bytes are not UTF-8 text or exceed `NORMALIZE_LIMIT`.
pub fn file_hashes(bytes: &[u8]) -> (String, String) {
    file_hashes_within(bytes, NORMALIZE_LIMIT)
}

/// `file_hashes` with another size limit.
pub fn file_hashes_within(bytes: &[u8], limit: u64) -> (String, String) {
    let normalized = if bytes.len() as u64 <= limit {
        std::str::from_utf8(bytes)
            .map(normalized_sha256)
            .unwrap_or_default()
    } else {
        String::new()
    };
    (sha256_hex(bytes), normalized)
}
