//! Deposits files with DataCite metadata to a research archive.
#![forbid(unsafe_code)]

pub mod http;

use std::fmt;

/// A token. It never prints.
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Secret {
        Secret(s.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[derive(Debug)]
pub enum Error {
    /// The request cannot be sent as given. Nothing ran.
    Request(String),
    /// The program or the connection failed.
    Transport(String),
    /// The server answered with a status the step does not accept.
    /// `body` holds at most the first 2,000 chars of the response.
    Status {
        step: &'static str,
        status: u16,
        body: String,
    },
    /// The response has an unexpected shape.
    Protocol {
        step: &'static str,
        what: String,
    },
    /// The server's copy differs from ours.
    Mismatch {
        step: &'static str,
        what: String,
    },
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Request(m) => write!(f, "bad request: {m}"),
            Error::Transport(m) => write!(f, "transport failed: {m}"),
            Error::Status { step, status, body } => {
                let head: String = body.chars().take(200).collect();
                write!(f, "{step}: status {status}: {head}")
            }
            Error::Protocol { step, what } => write!(f, "{step}: unexpected response: {what}"),
            Error::Mismatch { step, what } => write!(f, "{step}: mismatch: {what}"),
            Error::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e)
    }
}
