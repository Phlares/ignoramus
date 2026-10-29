//! Seeds: every random draw derives from `SHA-256(spec_hash || path)`.
use sha2::{Digest, Sha256};

pub fn seed(spec_hash: &[u8; 32], path: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(spec_hash);
    h.update(path.as_bytes());
    h.finalize().into()
}
