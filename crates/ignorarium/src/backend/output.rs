//! The hash of a folder, the same at every isolation level: a row's output and the payload.
//! The rows come from the guest crate, so the level 2 agent, the level 2 host and levels 0
//! and 1 hash with one function.
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub use ignorarium_guest::tree::{TreeRow, hash_rows, tree_hash as output_hash, tree_rows};

/// SHA-256 of a regular file, streamed. `None` when the path is no longer a
/// regular file by the time it is open (swapped for a FIFO or a link).
pub(crate) fn file_digest(path: &Path) -> io::Result<Option<String>> {
    let Some(mut f) = open_regular(path)? else {
        return Ok(None);
    };
    let mut h = Sha256::new();
    io::copy(&mut f, &mut h)?;
    Ok(Some(hex::encode(h.finalize())))
}

/// The regular file at `path`, open for reading, without following a link or blocking on
/// a FIFO. `None` when the path names anything else.
pub(crate) fn open_regular(path: &Path) -> io::Result<Option<fs::File>> {
    let opened = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path);
    let f = match opened {
        Ok(f) => f,
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(f.metadata()?.is_file().then_some(f))
}
