//! Where an input comes from: the content store when the spec pins a stored digest, else
//! the file under the study root. Every reader of an input goes through `resolve`.
use crate::prior::file_hashes;
use crate::rules::{input_bytes, input_path};
use crate::store::{Meta, Store};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// The store holds the blob; the digest is its pin.
    Stored { digest: String, meta: Meta },
    /// A regular file under the study root, with its hashes.
    File { exact: String, normalized: String },
    /// The path is safe and names no regular file.
    Missing,
    /// The path could leave the study root.
    Unsafe,
}

/// `Stored` when `sha256` is set and the store has its meta; else the file at `path`.
/// A store whose meta cannot be read counts as not holding the digest.
pub fn resolve(store: &Store, root: &Path, path: &str, sha256: &str) -> Found {
    if !sha256.is_empty()
        && let Ok(Some(meta)) = store.meta(sha256)
    {
        return Found::Stored {
            digest: sha256.to_string(),
            meta,
        };
    }
    if input_path(root, path).is_none() {
        return Found::Unsafe;
    }
    match input_bytes(root, path) {
        Some(bytes) => {
            let (exact, normalized) = file_hashes(&bytes);
            Found::File { exact, normalized }
        }
        None => Found::Missing,
    }
}

/// `(exact, normalized)` SHA-256 of the input, or `None` when there is none to read.
pub fn hashes(found: &Found) -> Option<(String, String)> {
    match found {
        Found::Stored { digest, meta } => Some((digest.clone(), meta.normalized_sha256.clone())),
        Found::File { exact, normalized } => Some((exact.clone(), normalized.clone())),
        Found::Missing | Found::Unsafe => None,
    }
}
