//! SHA-256 manifest in `sha256sum` format.
use crate::{Error, Problem, tree};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

pub const NAME: &str = "MANIFEST.sha256";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// Hashes a file in 64 KiB chunks. Returns the hex digest and the byte count.
pub fn sha256_file(path: &Path) -> io::Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), size))
}

pub fn sha256_bytes(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

/// Walks `dir` and hashes every file. Refuses when the walk finds an unsafe entry.
pub fn scan(dir: &Path) -> Result<Vec<Entry>, Error> {
    let (files, problems) = tree::walk(dir)?;
    if !problems.is_empty() {
        return Err(Error::Refused {
            problems,
            warnings: vec![],
        });
    }
    files
        .into_iter()
        .map(|path| {
            let (sha256, size) = sha256_file(&dir.join(&path))?;
            Ok(Entry { path, sha256, size })
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
    Problem {
        code: "E-PUB-015",
        path: tree::shown(path),
        message: message.to_string(),
        instruction: "Rebuild the bundle; never edit a built bundle.".to_string(),
    }
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
    for path in &files {
        if path == NAME {
            continue;
        }
        match listed.remove(path) {
            None => problems.push(mismatch(path, "not in the manifest")),
            Some(hash) => {
                if sha256_file(&dir.join(path))?.0 != hash {
                    problems.push(mismatch(path, "hash differs from the manifest"));
                }
            }
        }
    }
    for path in listed.keys() {
        problems.push(mismatch(path, "listed in the manifest but missing"));
    }
    if problems.is_empty() {
        Ok(sha256_bytes(&bytes))
    } else {
        Err(Error::Refused {
            problems,
            warnings: vec![],
        })
    }
}
