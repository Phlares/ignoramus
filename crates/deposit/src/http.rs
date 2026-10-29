//! HTTP transport. `Curl` runs the `curl` program and puts every value on its stdin.
use crate::{Error, Secret};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    None,
    Json(Vec<u8>),
    File(PathBuf),
}

#[derive(Debug)]
pub struct Request<'a> {
    pub method: Method,
    pub url: String,
    pub token: Option<&'a Secret>,
    pub body: Body,
    pub keep_body: bool,
    /// Follow up to 3 redirects. Only for requests that send no body and change nothing.
    pub follow: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub sha256: String,
    pub size: u64,
}

pub trait Transport {
    fn send(&self, req: &Request) -> Result<Response, Error>;
}

impl<T: Transport + ?Sized> Transport for &T {
    fn send(&self, req: &Request) -> Result<Response, Error> {
        (**self).send(req)
    }
}

pub struct Curl {
    program: PathBuf,
}

const KEEP_LIMIT: usize = 64 * 1024 * 1024;
/// curl aborts only a stalled transfer: one slower than 1 KiB/s for 2 minutes, or a
/// connection not made in 30 s. A long upload that keeps moving is never cut.
const LIMITS: &str = "connect-timeout = 30\nspeed-limit = 1024\nspeed-time = 120\n";

impl Curl {
    pub fn new() -> Curl {
        Curl::with_program("curl")
    }

    pub fn with_program(p: impl Into<PathBuf>) -> Curl {
        Curl { program: p.into() }
    }
}

impl Default for Curl {
    fn default() -> Curl {
        Curl::new()
    }
}

/// A quoted config value. A line break or NUL would end the line early.
fn quote(v: &str) -> Result<String, Error> {
    if v.contains(['\n', '\r', '\0']) {
        return Err(Error::Request(
            "a value holds a line break or NUL".to_string(),
        ));
    }
    Ok(format!(
        "\"{}\"",
        v.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

fn config(req: &Request) -> Result<String, Error> {
    let method = req.method.as_str();
    // curl drops a custom Authorization header when a redirect changes host (curl 7.58 and later).
    let follow = if req.follow {
        "location\nmax-redirs = 3\n"
    } else {
        ""
    };
    let mut c = format!(
        "silent\nshow-error\ngloboff\nproto = \"=https\"\nproto-redir = \"=https\"\n{follow}{LIMITS}url = {}\nrequest = \"{method}\"\n",
        quote(&req.url)?
    );
    if let Some(t) = req.token {
        let h = quote(&format!("Authorization: Bearer {}", t.expose()))?;
        c.push_str(&format!("header = {h}\n"));
    }
    match &req.body {
        Body::None => {}
        Body::Json(b) => {
            if !matches!(b.first(), Some(b'{' | b'[')) {
                return Err(Error::Request("a JSON body starts with { or [".to_string()));
            }
            let text = std::str::from_utf8(b)
                .map_err(|_| Error::Request("a JSON body is not UTF-8".to_string()))?;
            c.push_str("header = \"Content-Type: application/json\"\n");
            c.push_str(&format!("data-binary = {}\n", quote(text)?));
        }
        Body::File(p) => {
            let text = p
                .to_str()
                .ok_or_else(|| Error::Request("a file path is not UTF-8".to_string()))?;
            c.push_str("header = \"Content-Type: application/octet-stream\"\n");
            c.push_str(&format!("upload-file = {}\n", quote(text)?));
        }
    }
    c.push_str("write-out = \"\\n%{http_code}\"\n");
    Ok(c)
}

fn no_status() -> Error {
    Error::Transport("curl output has no status line".to_string())
}

impl Transport for Curl {
    fn send(&self, req: &Request) -> Result<Response, Error> {
        let cfg = config(req)?;
        let mut child = Command::new(&self.program)
            .args(["--config", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Transport(format!("cannot run {}: {e}", self.program.display())))?;
        let mut err_pipe = child.stderr.take().expect("stderr is piped");
        let err_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = err_pipe.read_to_end(&mut buf);
            buf
        });
        // A curl that exits early closes the pipe; its exit code explains why.
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let _ = stdin.write_all(cfg.as_bytes());
        drop(stdin);

        let mut out = child.stdout.take().expect("stdout is piped");
        let mut hasher = Sha256::new();
        let (mut body, mut size) = (Vec::new(), 0u64);
        let mut pending: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let n = match out.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Error::Io(e));
                }
            };
            pending.extend_from_slice(&chunk[..n]);
            if pending.len() > 4 {
                let cut = pending.len() - 4;
                hasher.update(&pending[..cut]);
                size += cut as u64;
                if req.keep_body {
                    body.extend_from_slice(&pending[..cut]);
                }
                pending.drain(..cut);
            }
            if body.len() > KEEP_LIMIT {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Transport(
                    "response body is larger than 64 MiB".to_string(),
                ));
            }
        }
        let status = child.wait()?;
        let stderr = err_thread.join().unwrap_or_default();
        if !status.success() {
            let code = status
                .code()
                .map_or("a signal".to_string(), |c| c.to_string());
            return Err(Error::Transport(format!(
                "curl exited with {code}: {}",
                String::from_utf8_lossy(&stderr).trim()
            )));
        }
        // At most 3 bytes follow the newline, so only 100 to 599 written as three digits passes.
        let [b'\n', digits @ ..] = pending.as_slice() else {
            return Err(no_status());
        };
        let status = std::str::from_utf8(digits)
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|s| (100..=599).contains(s))
            .ok_or_else(no_status)?;
        Ok(Response {
            status,
            body,
            sha256: hex::encode(hasher.finalize()),
            size,
        })
    }
}
