//! Earlier-use lookup: has a data file already been seen by an earlier study?
use crate::canon::sha256_hex;

pub trait PriorUse {
    /// The id of the earlier log entry that used this data, if any.
    /// `normalized_sha256` is empty when the file is not UTF-8 text.
    fn used(&self, exact_sha256: &str, normalized_sha256: &str) -> Option<String>;
}

/// A log with no entries.
pub struct NoPriorUse;

impl PriorUse for NoPriorUse {
    fn used(&self, _: &str, _: &str) -> Option<String> {
        None
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
