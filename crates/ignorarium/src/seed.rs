//! Seeds: every random draw derives from `SHA-256(spec_hash || path)`.
use sha2::{Digest, Sha256};

/// The seed of schedule row `index`, as hex: `seed(spec_hash, "row/<index>")`.
pub fn row_hex(spec_hash: &[u8; 32], index: u64) -> String {
    hex::encode(seed(spec_hash, &format!("row/{index}")))
}

pub fn seed(spec_hash: &[u8; 32], path: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(spec_hash);
    h.update(path.as_bytes());
    h.finalize().into()
}
