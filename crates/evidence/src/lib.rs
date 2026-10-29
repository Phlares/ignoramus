//! Sequential evidence statistics: e-processes, an online false-discovery stream, a tree split for spawned tests and scan p-values.
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

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    Parameter(&'static str),
    NotFinite,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tail {
    Upper,
    Lower,
    Both,
}
