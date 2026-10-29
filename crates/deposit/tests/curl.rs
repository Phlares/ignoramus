mod common;
use common::write_exec;
use deposit::http::{Body, Curl, Method, Request, Transport};
use deposit::{Error, Secret};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// A fake `curl` that saves its arguments and stdin beside itself, then runs `tail_script`.
fn fake(dir: &Path, tail_script: &str) -> PathBuf {
    let path = dir.join("curl");
    let text = format!(
        "#!/bin/sh\nd=$(dirname \"$0\")\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$d/argv\"\ncat > \"$d/stdin\"\n{tail_script}\n"
    );
    write_exec(&path, &text);
    path
}

fn req<'a>(url: &str, token: Option<&'a Secret>, body: Body, keep: bool) -> Request<'a> {
    Request {
        method: Method::Post,
        url: url.to_string(),
        token,
        body,
        keep_body: keep,
        follow: true,
    }
}

fn head() -> String {
    [
        "silent",
        "show-error",
        "globoff",
        "proto = \"=https\"",
        "proto-redir = \"=https\"",
        "location",
        "max-redirs = 3",
        "connect-timeout = 30",
        "speed-limit = 1024",
        "speed-time = 120",
        "",
    ]
    .join("\n")
}

#[test]
fn config_goes_on_stdin_not_argv() {
    let dir = TempDir::new().unwrap();
    let curl = Curl::with_program(fake(dir.path(), r"printf '\n201'"));
    let tok = Secret::new("tok123");
    let r = req(
        "https://sandbox.zenodo.org/api/x",
        Some(&tok),
        Body::Json(br#"{"a":"b\\c"}"#.to_vec()),
        true,
    );
    curl.send(&r).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("argv")).unwrap(),
        "--config\n-\n"
    );
    let want = format!(
        "{}url = \"https://sandbox.zenodo.org/api/x\"\nrequest = \"POST\"\nheader = \"Authorization: Bearer tok123\"\nheader = \"Content-Type: application/json\"\ndata-binary = \"{{\\\"a\\\":\\\"b\\\\\\\\c\\\"}}\"\nwrite-out = \"\\n%{{http_code}}\"\n",
        head()
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("stdin")).unwrap(),
        want
    );
}

#[test]
fn file_body_uses_upload_file() {
    let dir = TempDir::new().unwrap();
    let curl = Curl::with_program(fake(dir.path(), r"printf '\n200'"));
    let mut r = req(
        "https://sandbox.zenodo.org/f",
        None,
        Body::File(PathBuf::from("/data/a b.zip")),
        false,
    );
    r.method = Method::Put;
    curl.send(&r).unwrap();
    let want = format!(
        "{}url = \"https://sandbox.zenodo.org/f\"\nrequest = \"PUT\"\nheader = \"Content-Type: application/octet-stream\"\nupload-file = \"/data/a b.zip\"\nwrite-out = \"\\n%{{http_code}}\"\n",
        head()
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("stdin")).unwrap(),
        want
    );

    let mut g = req("https://sandbox.zenodo.org/g", None, Body::None, false);
    g.method = Method::Get;
    curl.send(&g).unwrap();
    let want = format!(
        "{}url = \"https://sandbox.zenodo.org/g\"\nrequest = \"GET\"\nwrite-out = \"\\n%{{http_code}}\"\n",
        head()
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("stdin")).unwrap(),
        want
    );
}

#[test]
fn status_and_body_split_at_the_tail() {
    let dir = TempDir::new().unwrap();
    let curl = Curl::with_program(fake(dir.path(), r"printf 'line1\nline2\n201'"));
    let r = curl
        .send(&req("https://h/x", None, Body::None, true))
        .unwrap();
    assert_eq!(r.status, 201);
    assert_eq!(r.body, b"line1\nline2");
    assert_eq!(r.size, 11);
    assert_eq!(r.sha256, hex::encode(Sha256::digest(b"line1\nline2")));
}

#[test]
fn large_body_hashes_without_keeping() {
    let dir = TempDir::new().unwrap();
    let curl = Curl::with_program(fake(
        dir.path(),
        r"head -c 1000000 /dev/zero | tr '\000' x; printf '\n200'",
    ));
    let r = curl
        .send(&req("https://h/x", None, Body::None, false))
        .unwrap();
    assert_eq!(r.status, 200);
    assert!(r.body.is_empty());
    assert_eq!(r.size, 1_000_000);
    assert_eq!(r.sha256, hex::encode(Sha256::digest(vec![b'x'; 1_000_000])));
}

#[test]
fn curl_failure_is_a_transport_error() {
    let dir = TempDir::new().unwrap();
    let curl = Curl::with_program(fake(
        dir.path(),
        "echo 'Could not resolve host' >&2\nexit 6",
    ));
    let e = curl
        .send(&req("https://h/x", None, Body::None, true))
        .unwrap_err();
    match e {
        Error::Transport(m) => assert_eq!(m, "curl exited with 6: Could not resolve host"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn short_or_bad_tail_is_a_transport_error() {
    for out in ["20", "abc\\n2x1"] {
        let dir = TempDir::new().unwrap();
        let curl = Curl::with_program(fake(dir.path(), &format!("printf '{out}'")));
        match curl.send(&req("https://h/x", None, Body::None, true)) {
            Err(Error::Transport(m)) => assert_eq!(m, "curl output has no status line"),
            other => panic!("{out}: {other:?}"),
        }
    }
}

#[test]
fn line_breaks_are_refused_before_running() {
    let dir = TempDir::new().unwrap();
    let curl = Curl::with_program(fake(dir.path(), r"printf '\n200'"));
    let bad = [
        req("https://x\ny", None, Body::None, true),
        req("https://x\ry", None, Body::None, true),
        req("https://x\0y", None, Body::None, true),
        req("https://x", None, Body::Json(b"{\n}".to_vec()), true),
        req("https://x", None, Body::Json(b"nope".to_vec()), true),
    ];
    for r in &bad {
        assert!(matches!(curl.send(r), Err(Error::Request(_))), "{r:?}");
    }
    let tok = Secret::new("a\nb");
    let r = req("https://x", Some(&tok), Body::None, true);
    assert!(matches!(curl.send(&r), Err(Error::Request(_))));
    assert!(!dir.path().join("stdin").exists());
}

#[test]
fn secrets_never_print() {
    let tok = Secret::new("tok123");
    assert_eq!(format!("{tok:?}"), "Secret(<redacted>)");
    assert_eq!(tok.expose(), "tok123");
    let r = req("https://x", Some(&tok), Body::None, true);
    assert!(!format!("{r:?}").contains("tok123"));
}

#[test]
fn a_status_outside_100_to_599_is_not_a_response() {
    for tail in ["000", "099", "600", "+99", "2 0"] {
        let dir = TempDir::new().unwrap();
        let curl = Curl::with_program(fake(dir.path(), &format!("printf '\n{tail}'")));
        match curl.send(&req("https://h/x", None, Body::None, true)) {
            Err(Error::Transport(m)) => assert_eq!(m, "curl output has no status line"),
            other => panic!("{tail}: {other:?}"),
        }
    }
    for (tail, want) in [("100", 100), ("599", 599)] {
        let dir = TempDir::new().unwrap();
        let curl = Curl::with_program(fake(dir.path(), &format!("printf '\n{tail}'")));
        let r = curl
            .send(&req("https://h/x", None, Body::None, true))
            .unwrap();
        assert_eq!(r.status, want);
    }
}

#[test]
fn the_keep_body_cap_is_64_mib() {
    const CAP: usize = 64 * 1024 * 1024;
    for (len, kept) in [(CAP, true), (CAP + 1, false)] {
        let dir = TempDir::new().unwrap();
        let script = format!("head -c {len} /dev/zero | tr '\000' x; printf '\n200'");
        let curl = Curl::with_program(fake(dir.path(), &script));
        let got = curl.send(&req("https://h/x", None, Body::None, true));
        if kept {
            let r = got.unwrap();
            assert_eq!(r.body.len(), CAP);
            assert_eq!(r.size, CAP as u64);
        } else {
            assert!(matches!(got, Err(Error::Transport(_))), "{got:?}");
        }
    }
}

#[test]
fn a_nul_token_or_a_broken_file_path_runs_nothing() {
    let nul = Secret::new("a\0b");
    let cr = Secret::new("a\rb");
    let cases = [
        req("https://x", Some(&nul), Body::None, true),
        req("https://x", Some(&cr), Body::None, true),
        req("https://x", None, Body::File(PathBuf::from("/a\nb")), true),
        req("https://x", None, Body::File(PathBuf::from("/a\rb")), true),
    ];
    for r in &cases {
        let dir = TempDir::new().unwrap();
        let curl = Curl::with_program(fake(dir.path(), r"printf '\n200'"));
        assert!(matches!(curl.send(r), Err(Error::Request(_))), "{r:?}");
        assert!(!dir.path().join("stdin").exists(), "{r:?}");
    }
}

#[test]
fn a_missing_program_is_a_transport_error() {
    let dir = TempDir::new().unwrap();
    let curl = Curl::with_program(dir.path().join("absent"));
    match curl.send(&req("https://h/x", None, Body::None, true)) {
        Err(Error::Transport(m)) => assert!(m.starts_with("cannot run "), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn redirects_are_followed_only_when_asked() {
    for follow in [true, false] {
        let dir = TempDir::new().unwrap();
        let curl = Curl::with_program(fake(dir.path(), r"printf '\n200'"));
        let mut r = req("https://sandbox.zenodo.org/f", None, Body::None, false);
        r.follow = follow;
        curl.send(&r).unwrap();
        let cfg = std::fs::read_to_string(dir.path().join("stdin")).unwrap();
        let want = if follow {
            head()
        } else {
            head().replace("location\nmax-redirs = 3\n", "")
        };
        assert!(cfg.starts_with(&want), "{follow}: {cfg}");
        assert_eq!(cfg.contains("location"), follow);
        assert_eq!(cfg.contains("max-redirs"), follow);
    }
}
