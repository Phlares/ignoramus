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

impl Problem {
    pub(crate) fn new(
        code: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
        instruction: impl Into<String>,
    ) -> Problem {
        Problem {
            code,
            path: path.into(),
            message: message.into(),
            instruction: instruction.into(),
        }
    }
}

/// `Ok` when there is no problem, else one refusal that lists them all.
pub(crate) fn refuse_any(problems: Vec<Problem>) -> Result<(), Error> {
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Error::Refused {
            problems,
            warnings: Vec::new(),
        })
    }
}

pub(crate) fn blank(s: &str) -> bool {
    s.trim().is_empty()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn p(code: &'static str, path: &str) -> Problem {
        Problem::new(code, path, "is wrong", "Fix it.")
    }

    #[test]
    fn a_problem_prints_code_path_message_and_instruction() {
        assert_eq!(
            p("E-PUB-001", "a/b").to_string(),
            "E-PUB-001 a/b: is wrong. Fix it."
        );
    }

    #[test]
    fn a_refusal_prints_one_problem_per_line() {
        let one = Error::Refused {
            problems: vec![p("E-PUB-001", "a")],
            warnings: vec![p("W-PUB-001", "w")],
        };
        assert_eq!(one.to_string(), "E-PUB-001 a: is wrong. Fix it.");
        let three = Error::Refused {
            problems: vec![
                p("E-PUB-001", "a"),
                p("E-PUB-002", "b"),
                p("E-PUB-003", "c"),
            ],
            warnings: Vec::new(),
        };
        assert_eq!(
            three.to_string(),
            "E-PUB-001 a: is wrong. Fix it.\nE-PUB-002 b: is wrong. Fix it.\nE-PUB-003 c: is wrong. Fix it."
        );
    }

    #[test]
    fn an_io_error_prints_its_cause() {
        let e = Error::Io(std::io::Error::other("disk full"));
        assert_eq!(e.to_string(), "I/O error: disk full");
    }
}
