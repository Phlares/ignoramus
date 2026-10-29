//! Event log: one canonical JSON line per event.
use crate::files::append_line;
use serde::Serialize;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    /// RFC 3339 UTC; the caller fills it from `time::now_rfc3339`.
    pub ts: String,
    pub trace_id: String,
    pub span_id: Option<String>,
    pub name: String,
    pub level: String,
    pub attrs: serde_json::Value,
}

/// Append one event line under an exclusive lock.
pub fn emit(path: &Path, e: Event) -> Result<(), io::Error> {
    append_line(path, &e)
}

/// A study's trace id: the first 16 bytes of its spec hash, in hex.
pub fn trace_id(spec_hash: &[u8; 32]) -> String {
    hex::encode(&spec_hash[..16])
}

fn random_hex<const N: usize>() -> String {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("operating system random source");
    hex::encode(b)
}

/// 32 lowercase hex characters.
pub fn new_trace_id() -> String {
    random_hex::<16>()
}

/// 16 lowercase hex characters.
pub fn new_span_id() -> String {
    random_hex::<8>()
}
