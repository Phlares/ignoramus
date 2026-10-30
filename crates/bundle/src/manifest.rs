//! SHA-256 manifest in `sha256sum` format.
use crate::{Error, Problem, refuse_any, tree};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

pub const NAME: &str = "MANIFEST.sha256";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// Reads `reader` to its end in 64 KiB chunks and hands each chunk to `each`.
/// Returns the byte count.
pub(crate) fn each_chunk(
    reader: &mut impl Read,
    mut each: impl FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<u64> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(size);
        }
        each(&buf[..n])?;
        size += n as u64;
    }
}

/// Copies `reader` into `sink`. Returns the hex SHA-256 and the byte count.
pub fn sha256_copy(reader: &mut impl Read, sink: &mut impl Write) -> io::Result<(String, u64)> {
    let mut hasher = Sha256::new();
    let size = each_chunk(reader, |b| {
        hasher.update(b);
        sink.write_all(b)
    })?;
    Ok((hex::encode(hasher.finalize()), size))
}

pub fn sha256_bytes(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

/// Walks `dir` and hashes every file. Refuses when the walk finds an unsafe entry.
pub fn scan(dir: &Path) -> Result<Vec<Entry>, Error> {
    let (files, problems) = tree::walk(dir)?;
    refuse_any(problems)?;
    files
        .into_iter()
        .map(|f| {
            let (sha256, size) = sha256_copy(&mut tree::open(dir, &f)?, &mut io::sink())?;
            Ok(Entry {
                path: f.path,
                sha256,
                size,
            })
        })
        .collect()
}

pub fn render(entries: &[Entry]) -> String {
    entries
        .iter()
        .map(|e| format!("{}  {}\n", e.sha256, e.path))
        .collect()
}

fn mismatch(path: &str, message: &str) -> Problem {
    Problem::new(
        "E-PUB-015",
        tree::shown(path),
        message,
        "Rebuild the bundle; never edit a built bundle.",
    )
}

/// Verifies `dir` against `dir/MANIFEST.sha256`. Returns the SHA-256 of the manifest bytes.
pub fn check(dir: &Path) -> Result<String, Error> {
    let bytes = fs::read(dir.join(NAME))?;
    let text = String::from_utf8_lossy(&bytes);
    let mut listed = BTreeMap::new();
    for line in text.lines() {
        if let Some((hash, path)) = line.split_once("  ") {
            listed.insert(path.to_string(), hash.to_string());
        }
    }
    let (files, mut problems) = tree::walk(dir)?;
    for found in &files {
        let path = &found.path;
        if path == NAME {
            continue;
        }
        match listed.remove(path) {
            None => problems.push(mismatch(path, "not in the manifest")),
            Some(hash) => {
                if sha256_copy(&mut tree::open(dir, found)?, &mut io::sink())?.0 != hash {
                    problems.push(mismatch(path, "hash differs from the manifest"));
                }
            }
        }
    }
    for path in listed.keys() {
        problems.push(mismatch(path, "listed in the manifest but missing"));
    }
    refuse_any(problems)?;
    Ok(sha256_bytes(&bytes))
}
