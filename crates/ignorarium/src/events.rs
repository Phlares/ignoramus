//! Event log: one canonical JSON line per event.
use crate::canon::canonical_of;
use crate::files::{Tail, locked_append};
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

/// Append one event line under an exclusive lock. A log that ends in a
/// partial line gets a LF first, so the fragment stays its own line.
pub fn emit(path: &Path, e: Event) -> Result<(), io::Error> {
    let mut line = canonical_of(&e)?;
    line.push(b'\n');
    locked_append(path, |tail| {
        if matches!(tail, Tail::Partial(_)) {
            line.insert(0, b'\n');
        }
        Ok::<_, io::Error>(line)
    })
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
