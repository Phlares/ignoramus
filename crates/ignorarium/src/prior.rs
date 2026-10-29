//! Earlier-use lookup: has a data file already been seen by an earlier study?
use crate::canon::sha256_hex;

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
    /// The id of the earlier log entry that used this data, if any.
    /// `normalized_sha256` is empty when the file is not UTF-8 text.
    fn used(
        &self,
        exact_sha256: &str,
        normalized_sha256: &str,
    ) -> Result<Option<String>, PriorError>;
}

/// A log with no entries.
pub struct NoPriorUse;

impl PriorUse for NoPriorUse {
    fn used(&self, _: &str, _: &str) -> Result<Option<String>, PriorError> {
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

/// `(exact, normalized)` SHA-256 of a file. The normalized hash is empty
/// when the bytes are not UTF-8 text.
pub fn file_hashes(bytes: &[u8]) -> (String, String) {
    let normalized = std::str::from_utf8(bytes)
        .map(normalized_sha256)
        .unwrap_or_default();
    (sha256_hex(bytes), normalized)
}
