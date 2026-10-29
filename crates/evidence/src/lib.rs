//! Sequential evidence statistics: e-processes, online false-discovery control, tree splits and scan p-values.
//!
//! The crate does no I/O and has no dependencies. Every random draw comes from an [`rng::Rng`] that the caller seeds.
#![forbid(unsafe_code)]
use std::fmt;

pub mod elond;
pub mod eprocess;
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

/// Which side of the null counts as evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tail {
    Upper,
    Lower,
    Both,
}
