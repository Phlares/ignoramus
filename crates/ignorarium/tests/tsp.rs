//! Tests for the RFC 3161 time-stamp reply parser.
use ignorarium::tsp::{self, SHA256};
use sha2::{Digest, Sha256};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tsp");

fn read(name: &str) -> Vec<u8> {
    std::fs::read(format!("{DIR}/{name}")).unwrap()
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

#[test]
fn parses_a_reply_openssl_made() {
    let reply = tsp::parse(&read("local.tsr")).unwrap();
    assert!(reply.granted);
    assert_eq!(reply.digest_alg, SHA256);
    assert_eq!(reply.digest, sha256(&read("local.txt")));
    assert_eq!(
        reply.authority.as_deref(),
        Some("CN=ignorarium test authority")
    );
    assert_eq!(reply.policy, "1.2.3.4.1");
    assert!(!reply.serial.is_empty());
    let at = reply.stamped_at.as_bytes();
    assert_eq!(at.len(), "2026-11-10T22:41:49.000Z".len());
    assert_eq!(at[10], b'T');
    assert_eq!(at.last(), Some(&b'Z'));
}

#[test]
fn granted_with_mods_is_granted() {
    let d = [7u8; 32];
    let mut der = tsp::fixture(&d, 1_794_350_509, None);
    // The status INTEGER, granted (0), is the first such field; granted with mods is 1.
    let status = [0x30, 0x03, 0x02, 0x01, 0x00];
    let at = der.windows(5).position(|w| w == status).unwrap();
    der[at + 4] = 0x01;
    let reply = tsp::parse(&der).unwrap();
    assert!(reply.granted);
    assert_eq!(reply.digest, d);
}

#[test]
fn fixture_round_trips() {
    let d = [7u8; 32];
    let reply = tsp::parse(&tsp::fixture(&d, 1_794_350_509, Some("CN=T"))).unwrap();
    assert!(reply.granted);
    assert_eq!(reply.digest_alg, SHA256);
    assert_eq!(reply.digest, d);
    assert_eq!(reply.stamped_at, "2026-11-10T22:41:49.000Z");
    assert_eq!(reply.authority, Some("CN=T".to_string()));
    let reply = tsp::parse(&tsp::fixture(&d, 1_794_350_509, None)).unwrap();
    assert_eq!(reply.authority, None);
}

#[test]
fn a_rejection_parses_as_not_granted() {
    // TimeStampResp { status: { status: rejection(2) } }, no token.
    let reply = tsp::parse(&[0x30, 0x05, 0x30, 0x03, 0x02, 0x01, 0x02]).unwrap();
    assert!(!reply.granted);
    assert!(reply.digest.is_empty());
    assert_eq!(reply.authority, None);
}

#[test]
fn rubbish_is_an_error_not_a_panic() {
    let mut random = [0u8; 10];
    getrandom::fill(&mut random).unwrap();
    let html = b"<!doctype html><html><body>502 Bad Gateway</body></html>".to_vec();
    let mut inputs = vec![Vec::new(), random.to_vec(), html];
    let tsr = read("local.tsr");
    inputs.extend((0..tsr.len()).step_by(97).map(|n| tsr[..n].to_vec()));
    for input in inputs {
        assert!(tsp::parse(&input).is_err(), "{} bytes parsed", input.len());
    }
}

#[test]
fn a_real_reply_parses() {
    let (Some(tsr), Some(file)) = (
        std::env::var_os("IGNORARIUM_REAL_TSR"),
        std::env::var_os("IGNORARIUM_REAL_TSR_FILE"),
    ) else {
        println!("skip real reply: IGNORARIUM_REAL_TSR and IGNORARIUM_REAL_TSR_FILE are unset");
        return;
    };
    let reply = tsp::parse(&std::fs::read(tsr).unwrap()).unwrap();
    println!(
        "authority={:?} stamped_at={} policy={} serial={}",
        reply.authority, reply.stamped_at, reply.policy, reply.serial
    );
    assert!(reply.granted);
    assert_eq!(reply.digest, sha256(&std::fs::read(file).unwrap()));
    assert!(reply.authority.is_some());
}
