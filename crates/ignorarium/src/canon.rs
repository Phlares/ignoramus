//! Canonical JSON and SHA-256 hashing.
use sha2::{Digest, Sha256};

/// Serialization failure. `serde_json::Value` holds only finite numbers,
/// so this is not reachable for values built from it.
#[derive(Debug)]
pub struct CanonError(serde_json::Error);

impl std::fmt::Display for CanonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "canonical JSON failed: {}", self.0)
    }
}

impl std::error::Error for CanonError {}

/// Compact JSON with object keys sorted.
/// Key order comes from `serde_json::Map`, which is a `BTreeMap` here:
/// the `preserve_order` feature must stay off.
pub fn to_canonical(v: &serde_json::Value) -> Result<Vec<u8>, CanonError> {
    serde_json::to_vec(v).map_err(CanonError)
}

/// `to_canonical` of any serializable value.
pub fn canonical_of<T: serde::Serialize>(v: &T) -> std::io::Result<Vec<u8>> {
    let v = serde_json::to_value(v).map_err(std::io::Error::other)?;
    to_canonical(&v).map_err(std::io::Error::other)
}

/// `canonical_of` and a final LF: one line of a JSON Lines file.
pub fn canonical_line<T: serde::Serialize>(v: &T) -> std::io::Result<Vec<u8>> {
    let mut line = canonical_of(v)?;
    line.push(b'\n');
    Ok(line)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
