//! What identifies this box: the rules it embeds and the binary that runs. Observation only:
//! no decision, hash or comparison of a study reads it.
use crate::canon::{sha256_file, sha256_hex};
use crate::telemetry::Reading;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::OnceLock;

/// The hash of both embedded rule files. Each part carries its byte length as a `u64`
/// big-endian prefix, so no text moves across the boundary between the two files.
pub fn rules_sha256() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        let mut bytes = Vec::new();
        for part in [crate::rules::RULES_TOML, crate::scan::SCAN_TOML] {
            bytes.extend((part.len() as u64).to_be_bytes());
            bytes.extend(part.as_bytes());
        }
        format!("sha256:{}", sha256_hex(&bytes))
    })
}

/// The hash of the running executable, or why it cannot be read.
pub fn binary_sha256() -> &'static Reading<String> {
    static HASH: OnceLock<Reading<String>> = OnceLock::new();
    HASH.get_or_init(|| match sha256_file("/proc/self/exe".as_ref()) {
        Ok(hex) => Reading::Read(format!("sha256:{hex}")),
        Err(e) => Reading::Unavailable(format!("/proc/self/exe: {e}")),
    })
}

/// `binary_sha256` is a `Value`, so an unavailable reading keeps its `{"unavailable": why}` form.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BoxIdentity {
    pub rules_sha256: String,
    pub binary_sha256: Value,
}

pub fn current() -> BoxIdentity {
    BoxIdentity {
        rules_sha256: rules_sha256().into(),
        binary_sha256: serde_json::to_value(binary_sha256()).unwrap_or(Value::Null),
    }
}

/// True only when both sides are hash strings and equal.
pub fn same_binary(a: &Value, b: &Value) -> bool {
    matches!((a, b), (Value::String(x), Value::String(y)) if x == y)
}
