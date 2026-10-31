//! The hash of a folder, the same at every isolation level: a row's output and the payload.
//! The rows come from the guest crate, so the level 2 agent, the level 2 host and levels 0
//! and 1 hash with one function, and one test decides what a regular file is.
use std::io;
use std::path::Path;

pub use ignorarium_guest::tree::{
    TreeRow, hash_rows, open_regular, sha256_hex, tree_hash as output_hash, tree_rows,
};

/// SHA-256 of a regular file, streamed. `None` when the path is no longer a
/// regular file by the time it is open (swapped for a FIFO or a link).
pub(crate) fn file_digest(path: &Path) -> io::Result<Option<String>> {
    open_regular(path)?.map(sha256_hex).transpose()
}
