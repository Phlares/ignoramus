//! Builds a study's publish bundle: metadata, licences, manifest, zip.
//!
//! The crate has no network and no clock. The caller passes dates and versions.
#![forbid(unsafe_code)]
use std::fmt;

pub mod build;
pub mod citation;
pub mod licence;
pub mod manifest;
pub mod profile;
pub mod rocrate;
pub mod tree;
pub mod yaml;
pub mod zip;

pub use build::{Built, Facts, GENERATED, build, zip_name};

/// One refusal: a stable code, the path it concerns, what is wrong and how to fix it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub code: &'static str,
    pub path: String,
    pub message: String,
    pub instruction: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}: {}. {}",
            self.code, self.path, self.message, self.instruction
        )
    }
}

/// Why a call failed.
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// The input breaks a rule. Every problem is listed at once.
    Refused {
        problems: Vec<Problem>,
        warnings: Vec<Problem>,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Refused { problems, .. } => {
                for (i, p) in problems.iter().enumerate() {
                    if i > 0 {
                        f.write_str("\n")?;
                    }
                    write!(f, "{p}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
