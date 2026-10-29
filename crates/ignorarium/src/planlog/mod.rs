//! The plan log: a signed, append-only Merkle log of registered plans.
use crate::canon::{sha256_hex, to_canonical};
use crate::spec::model::Spec;
use serde_json::json;
use sha2::{Digest, Sha256};

pub mod merkle;
pub mod store;

/// Hex SHA-256 of `salt || spec_canonical`: binds a plan without revealing it.
pub fn commitment(salt: &[u8; 32], spec_canonical: &[u8]) -> String {
    hex::encode(
        Sha256::new()
            .chain_update(salt)
            .chain_update(spec_canonical)
            .finalize(),
    )
}

/// The first 16 hex chars of SHA-256 over the payload pin, the outcome
/// ids and units, and the hypothesis ids. Both lists are sorted, so
/// their order in the spec does not matter.
pub fn family(spec: &Spec) -> String {
    let mut outcomes: Vec<[&str; 2]> = spec
        .outcomes
        .iter()
        .map(|o| [o.id.as_str(), o.unit.as_str()])
        .collect();
    outcomes.sort_unstable();
    let mut hypotheses: Vec<&str> = spec
        .question
        .hypotheses
        .iter()
        .map(|h| h.id.as_str())
        .collect();
    hypotheses.sort_unstable();
    let v = json!({
        "payload": spec.pins.payload,
        "outcomes": outcomes,
        "hypotheses": hypotheses,
    });
    let bytes = to_canonical(&v).expect("a JSON value of strings and arrays always serializes");
    sha256_hex(&bytes)[..16].to_string()
}
