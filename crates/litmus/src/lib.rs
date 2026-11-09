//! A synthetic study with known answers, to test the box.
#![forbid(unsafe_code)]
use sha2::{Digest, Sha256};

/// Half-width of the noise around each condition's mean.
pub const SPREAD: f64 = 0.2;
/// Range of the outcome measure.
pub const RANGE: [f64; 2] = [-1.0, 1.0];
/// Margin of the claims. Every claim is "below 0.0".
pub const MARGIN: f64 = 0.0;

/// The true mean of a condition, or `None` for an unknown name.
pub fn mean_of(condition: &str) -> Option<f64> {
    match condition {
        "ref" | "edge" => Some(0.0),
        "good" => Some(-0.6),
        "bad" => Some(0.3),
        _ => None,
    }
}

/// The outcome for one unit of one cluster under one condition.
///
/// `ref` is exactly `0.0`, so a paired difference against it equals the other arm's value.
/// Other conditions add noise in `[-SPREAD, SPREAD)`, drawn from a hash of the three inputs.
pub fn value(cluster: &[u8], unit: u32, condition: &str) -> Option<f64> {
    let mean = mean_of(condition)?;
    if condition == "ref" {
        return Some(0.0);
    }
    let mut h = Sha256::new();
    h.update(cluster);
    h.update([0]);
    h.update(unit.to_be_bytes());
    h.update([0]);
    h.update(condition.as_bytes());
    let digest = h.finalize();
    let bits = u64::from_be_bytes(digest[..8].try_into().expect("digest has 32 bytes"));
    let u = bits as f64 / 2f64.powi(64);
    Some(mean + (u - 0.5) * 2.0 * SPREAD)
}

mod flags;
mod generate;
mod payload;
mod verify;
pub use flags::Flags;
pub use generate::{Shape, generate};
pub use payload::payload;
pub use verify::verify;
