//! Sequential evidence statistics: e-processes, online false-discovery control, tree splits, scan p-values, and the claim test with its power.
//!
//! The crate does no I/O and has no dependencies. Every random draw comes from an [`rng::Rng`] that the caller seeds.
#![forbid(unsafe_code)]
use std::fmt;

pub mod claim;
pub mod elond;
pub mod eprocess;
pub mod power;
pub mod rng;
pub mod scan;
pub mod sim;
pub mod tree;

/// Why a call refused its input.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// A parameter is invalid; the string names it.
    Parameter(&'static str),
    /// An observation, a difference or a null statistic is NaN or infinite.
    /// The message says "observation" for any of these.
    NotFinite,
    /// An observation lies outside the bounds of a `Bounded` score.
    OutOfBounds,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parameter(name) => write!(f, "invalid parameter: {name}"),
            Error::NotFinite => f.write_str("observation is not finite"),
            Error::OutOfBounds => f.write_str("observation is outside the declared bounds"),
        }
    }
}

impl std::error::Error for Error {}

/// Whether e-value `e` reaches 1 / `level`. A NaN never does, nor anything at a level of 0.
pub fn rejects(e: f64, level: f64) -> bool {
    level > 0.0 && e >= 1.0 / level
}

/// Which side of the null counts as evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tail {
    Upper,
    Lower,
    Both,
}
