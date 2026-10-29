// Each test file uses a different part of this module.
#![allow(dead_code)]
use ignorarium_guest::pack;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Writes an executable through a child process, so no write fd leaks into a fork (ETXTBSY).
pub fn write_exec(path: &Path, text: &str) {
    let mut child = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
}

/// `dir` as a pack, by the crate's writer.
pub fn packed(dir: &Path) -> Vec<u8> {
    let mut v = Vec::new();
    let n = pack::write(dir, &mut v, u64::MAX).unwrap();
    assert_eq!(n, v.len() as u64);
    v
}

/// A pack built by hand from entries.
pub fn pack(parts: &[Vec<u8>]) -> Vec<u8> {
    let mut v = b"IGNPACK1".to_vec();
    for p in parts {
        v.extend_from_slice(p);
    }
    v.push(b'e');
    v
}

pub fn entry(kind: u8, path: &[u8]) -> Vec<u8> {
    let mut v = vec![kind];
    v.extend_from_slice(&(path.len() as u32).to_be_bytes());
    v.extend_from_slice(path);
    v
}

pub fn file(path: &[u8], body: &[u8]) -> Vec<u8> {
    let mut v = entry(b'f', path);
    v.extend_from_slice(&(body.len() as u64).to_be_bytes());
    v.extend_from_slice(body);
    v
}

pub fn link(path: &[u8], target: &[u8]) -> Vec<u8> {
    let mut v = entry(b'l', path);
    v.extend_from_slice(&(target.len() as u32).to_be_bytes());
    v.extend_from_slice(target);
    v
}
