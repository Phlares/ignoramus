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

/// A DER element: `tag`, a definite length, then `content`.
fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let n = content.len();
    let mut out = vec![tag];
    match n {
        0..=0x7f => out.push(n as u8),
        0x80..=0xff => out.extend([0x81, n as u8]),
        _ => out.extend([0x82, (n >> 8) as u8, n as u8]),
    }
    out.extend(content);
    out
}

/// The header length and the content length of the DER element at the start of `der`.
fn header(der: &[u8]) -> (usize, usize) {
    match der[1] {
        n @ 0..=0x7f => (2, n as usize),
        long => {
            let k = (long & 0x7f) as usize;
            let n = der[2..2 + k].iter().fold(0, |n, &b| n << 8 | b as usize);
            (2 + k, n)
        }
    }
}

/// The elements inside the DER element `der`, each with its tag and length.
fn children(der: &[u8]) -> Vec<&[u8]> {
    let (h, n) = header(der);
    let mut rest = &der[h..h + n];
    let mut out = Vec::new();
    while !rest.is_empty() {
        let (h, n) = header(rest);
        let (one, after) = rest.split_at(h + n);
        out.push(one);
        rest = after;
    }
    out
}

/// `der` with the element at `path` replaced by what `edit` makes of it. Each step of `path`
/// picks a child; the elements around it get new lengths.
fn edit_at(der: &[u8], path: &[usize], edit: impl FnOnce(&[u8]) -> Vec<u8>) -> Vec<u8> {
    let Some((&at, path)) = path.split_first() else {
        return edit(der);
    };
    let mut kids: Vec<Vec<u8>> = children(der).into_iter().map(<[u8]>::to_vec).collect();
    kids[at] = edit_at(&kids[at], path, edit);
    tlv(der[0], &kids.concat())
}

/// Reply, token content, signed data, encapsulated content, its OCTET STRING, the TSTInfo.
const TST_INFO: [usize; 7] = [1, 1, 0, 2, 1, 0, 0];

/// `fixture`'s reply with its time field `time`, as GeneralizedTime text.
fn stamped_at(time: &[u8]) -> Vec<u8> {
    let reply = tsp::fixture(&[7u8; 32], 1_794_350_509, None);
    let path = [&TST_INFO[..], &[4]].concat();
    edit_at(&reply, &path, |old| {
        assert_eq!(old, tlv(0x18, b"20261110224149Z"));
        tlv(0x18, time)
    })
}

#[test]
fn a_time_with_a_fraction_of_a_second_parses_to_the_millisecond() {
    let at = |time: &[u8]| tsp::parse(&stamped_at(time)).map(|r| r.stamped_at);
    assert_eq!(at(b"20261110224149Z").unwrap(), "2026-11-10T22:41:49.000Z");
    assert_eq!(
        at(b"20261110224149.5Z").unwrap(),
        "2026-11-10T22:41:49.500Z"
    );
    assert_eq!(
        at(b"20261110224149.25Z").unwrap(),
        "2026-11-10T22:41:49.250Z"
    );
    assert_eq!(
        at(b"20261110224149.1239Z").unwrap(),
        "2026-11-10T22:41:49.123Z"
    );
    // DER writes no trailing zero, no empty fraction and no comma (X.690 11.7).
    for time in [
        &b"20261110224149.50Z"[..],
        b"20261110224149.Z",
        b"20261110224149,5Z",
    ] {
        let e = at(time).unwrap_err();
        assert!(e.to_string().starts_with("not a time-stamp reply"), "{e}");
    }
}

/// The openssl-made reply with its certificate set listing its one certificate `times` times.
fn certificate_times(times: usize) -> Vec<u8> {
    // Reply, token content, signed data, its certificate set.
    edit_at(&read("local.tsr"), &[1, 1, 0, 3], |set| {
        assert_eq!(set[0], 0xa0);
        let one = children(set);
        assert_eq!(one.len(), 1);
        tlv(0xa0, &one[0].repeat(times))
    })
}

#[test]
fn a_certificate_set_with_a_repeat_parses() {
    assert_eq!(certificate_times(1), read("local.tsr"));
    let reply = tsp::parse(&certificate_times(2)).unwrap();
    assert_eq!(
        reply.authority.as_deref(),
        Some("CN=ignorarium test authority")
    );
}

/// The openssl-made reply with its first signer info naming the certificate by key identifier.
fn named_by_key() -> Vec<u8> {
    // Reply, token content, signed data, signer infos, the first, its signer identifier.
    edit_at(&read("local.tsr"), &[1, 1, 0, 4, 0, 1], |sid| {
        assert_eq!(sid[0], 0x30);
        tlv(0x80, &[1, 2, 3, 4])
    })
}

#[test]
fn a_signer_named_by_key_identifier_gives_no_authority() {
    assert_eq!(
        tsp::parse(&read("local.tsr")).unwrap().authority.as_deref(),
        Some("CN=ignorarium test authority")
    );
    let reply = tsp::parse(&named_by_key()).unwrap();
    assert!(reply.granted);
    assert_eq!(reply.authority, None);
}

#[test]
fn a_tsa_that_is_not_a_directory_name_gives_no_authority() {
    // The `tsa` field, `[0]` explicit, holds a GeneralName: a URI `[6]` or an e-mail `[1]`.
    let reply = tsp::fixture(&[7u8; 32], 1_794_350_509, Some("CN=T"));
    let path = [&TST_INFO[..], &[5, 0]].concat();
    for (tag, value) in [(0x86, &b"http://tsa.test"[..]), (0x81, b"tsa@test")] {
        let der = edit_at(&reply, &path, |old| {
            assert_eq!(old[0], 0xa4);
            tlv(tag, value)
        });
        let reply = tsp::parse(&der).unwrap();
        assert!(reply.granted);
        assert_eq!(reply.authority, None, "{tag:#x}");
    }
}

/// `fixture`'s reply with its status INTEGER set to `status`.
fn with_status(status: u8) -> Vec<u8> {
    let reply = tsp::fixture(&[7u8; 32], 1_794_350_509, None);
    edit_at(&reply, &[0, 0], |_| tlv(0x02, &[status]))
}

#[test]
fn a_refusal_status_parses_and_an_unknown_one_does_not() {
    for status in 2..=5 {
        let reply = tsp::parse(&with_status(status)).unwrap();
        assert!(!reply.granted, "{status}");
        assert!(reply.digest.is_empty(), "{status}");
    }
    for status in [6, 7, 100] {
        let e = tsp::parse(&with_status(status)).unwrap_err();
        assert!(e.to_string().contains("unknown status"), "{status}: {e}");
    }
}

#[test]
fn a_time_that_is_not_utc_is_refused() {
    for time in [
        &b"20261110224149+0000"[..],
        b"20261110224149",
        b"20261110224149-0500",
    ] {
        let e = tsp::parse(&stamped_at(time)).unwrap_err();
        assert!(e.to_string().starts_with("not a time-stamp reply"), "{e}");
    }
}
