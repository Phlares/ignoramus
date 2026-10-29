//! Deposits files with DataCite metadata to a research archive.
#![forbid(unsafe_code)]

pub mod http;
pub mod zenodo;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_error_prints_its_text() {
        let cases = [
            (Error::Request("x".into()), "bad request: x"),
            (Error::Transport("x".into()), "transport failed: x"),
            (
                Error::Protocol {
                    step: "verify",
                    what: "x".into(),
                },
                "verify: unexpected response: x",
            ),
            (
                Error::Mismatch {
                    step: "verify",
                    what: "x".into(),
                },
                "verify: mismatch: x",
            ),
            (
                Error::Io(std::io::Error::other("disk full")),
                "io error: disk full",
            ),
        ];
        for (e, want) in cases {
            assert_eq!(e.to_string(), want);
        }
    }

    #[test]
    fn a_status_error_prints_the_first_200_chars_of_the_body() {
        let e = Error::Status {
            step: "create",
            status: 400,
            body: "\u{e9}".repeat(201),
        };
        assert_eq!(
            e.to_string(),
            format!("create: status 400: {}", "\u{e9}".repeat(200))
        );
    }
}
