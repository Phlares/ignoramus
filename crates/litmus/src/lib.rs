//! A synthetic study with known answers, to test the box.
#![forbid(unsafe_code)]
use sha2::{Digest, Sha256};

/// Half-width of the noise around each condition's mean.
pub const SPREAD: f64 = 0.2;
/// Range of the outcome measure.
pub const RANGE: [f64; 2] = [-1.0, 1.0];
/// Margin of the claims. Every claim is "below 0.0".
pub const MARGIN: f64 = 0.0;

/// The true mean of a condition and the half-width of its noise, or `None` for an unknown
/// name. `ref` has no noise, so a paired difference against it equals the other arm's value.
pub fn arm(condition: &str) -> Option<(f64, f64)> {
    match condition {
        "ref" => Some((0.0, 0.0)),
        "good" => Some((-0.6, SPREAD)),
        "bad" => Some((0.3, SPREAD)),
        "edge" => Some((0.0, SPREAD)),
        _ => None,
    }
}

/// The outcome for one unit of one cluster under one condition: the arm's mean plus noise in
/// `[-spread, spread]`, drawn from a hash of the three inputs. The upper end is reachable: the
/// u64 to f64 conversion rounds the top 2^10 values up to 2^64.
pub fn value(cluster: &[u8], unit: u32, condition: &str) -> Option<f64> {
    let (mean, spread) = arm(condition)?;
    let mut h = Sha256::new();
    h.update(cluster);
    h.update([0]);
    h.update(unit.to_be_bytes());
    h.update([0]);
    h.update(condition.as_bytes());
    let digest = h.finalize();
    let bits = u64::from_be_bytes(digest[..8].try_into().expect("digest has 32 bytes"));
    let u = bits as f64 / 2f64.powi(64);
    Some(mean + (u - 0.5) * 2.0 * spread)
}

mod generate;
mod payload;
mod verify;
pub use generate::{Shape, generate};
pub use payload::payload;
pub use verify::verify;
