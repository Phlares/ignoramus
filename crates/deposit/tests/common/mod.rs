//! Helpers shared by the deposit tests.
#![allow(dead_code)]
use std::path::Path;

/// Write an executable file without this process holding a write descriptor to it.
/// A thread that forks while such a descriptor is open hands it to its child, and an
/// `exec` of the file then fails with ETXTBSY. A child process writes the file instead.
pub fn write_exec(path: &Path, text: &str) {
    use std::io::Write;
    let mut child = std::process::Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success(), "cannot write {path:?}");
}

use deposit::Error;
use deposit::http::{Body, Method, Request, Response, Transport};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::VecDeque;

/// One request the fake saw.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub method: Method,
    pub url: String,
    pub token: Option<String>,
    pub body: Body,
    pub keep_body: bool,
}

/// A response with its hash and size computed from the bytes.
pub fn reply(status: u16, body: impl AsRef<[u8]>) -> Response {
    let body = body.as_ref().to_vec();
    Response {
        status,
        sha256: hex::encode(Sha256::digest(&body)),
        size: body.len() as u64,
        body,
    }
}

/// A response whose hash is scripted, as for a download whose body is not kept.
pub fn download(sha256: &str, size: u64) -> Response {
    Response {
        status: 200,
        body: Vec::new(),
        sha256: sha256.to_string(),
        size,
    }
}

/// A transport that records each request and returns scripted responses in order.
pub struct Fake {
    pub calls: RefCell<Vec<Call>>,
    script: RefCell<VecDeque<Response>>,
}

impl Fake {
    pub fn new(script: Vec<Response>) -> Fake {
        Fake {
            calls: RefCell::new(Vec::new()),
            script: RefCell::new(script.into()),
        }
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.borrow().clone()
    }
}

impl Transport for Fake {
    fn send(&self, req: &Request) -> Result<Response, Error> {
        self.calls.borrow_mut().push(Call {
            method: req.method,
            url: req.url.clone(),
            token: req.token.map(|t| t.expose().to_string()),
            body: req.body.clone(),
            keep_body: req.keep_body,
        });
        Ok(self
            .script
            .borrow_mut()
            .pop_front()
            .expect("the fake transport ran out of scripted responses"))
    }
}
