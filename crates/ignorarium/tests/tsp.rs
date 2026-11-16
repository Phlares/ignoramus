//! Tests for the RFC 3161 time-stamp reply parser.
use cms::cert::x509::der::asn1::OctetString;
use cms::cert::x509::der::{Any, Decode, Encode, Tag};
use cms::signed_data::SignedData;
use ignorarium::tsp::{self, SHA256};
use sha2::{Digest, Sha256};
use x509_tsp::TimeStampResp;

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

/// The content of the DER element `der`, without its tag and length.
fn content(der: &[u8]) -> &[u8] {
    match der[1] {
        n @ 0..=0x7f => &der[2..2 + n as usize],
        long => &der[2 + (long & 0x7f) as usize..],
    }
}

/// `reply` with its signed data replaced by the SEQUENCE whose content `edit` returns.
fn with_signed_data(reply: &[u8], edit: impl FnOnce(&SignedData) -> Vec<u8>) -> Vec<u8> {
    let mut resp = TimeStampResp::from_der(reply).unwrap();
    let token = resp.time_stamp_token.as_mut().unwrap();
    let signed: SignedData = token.content.decode_as().unwrap();
    token.content = Any::new(Tag::Sequence, edit(&signed)).unwrap();
    resp.to_der().unwrap()
}

/// `fixture`'s reply with its time field `time`, as GeneralizedTime text.
fn stamped_at(time: &[u8]) -> Vec<u8> {
    let whole = b"20261110224149Z";
    with_signed_data(&tsp::fixture(&[7u8; 32], 1_794_350_509, None), |signed| {
        let octets: OctetString = (signed.encap_content_info.econtent.as_ref())
            .unwrap()
            .decode_as()
            .unwrap();
        let tst = octets.as_bytes();
        let mut fields = content(tst).to_vec();
        let at = fields
            .windows(whole.len())
            .position(|w| w == whole)
            .unwrap();
        assert_eq!(fields[at - 2..at], [0x18, 0x0f]);
        fields.splice(at - 2..at + whole.len(), tlv(0x18, time));
        let mut signed = signed.clone();
        let tst = OctetString::new(tlv(0x30, &fields)).unwrap();
        signed.encap_content_info.econtent = Some(Any::encode_from(&tst).unwrap());
        content(&signed.to_der().unwrap()).to_vec()
    })
}

/// The DER crate reads a GeneralizedTime with no fraction of a second only.
#[test]
fn a_time_with_a_fraction_of_a_second_is_an_error() {
    let reply = tsp::parse(&stamped_at(b"20261110224149Z")).unwrap();
    assert_eq!(reply.stamped_at, "2026-11-10T22:41:49.000Z");
    let e = tsp::parse(&stamped_at(b"20261110224149.5Z")).unwrap_err();
    assert!(e.to_string().starts_with("not a time-stamp reply"), "{e}");
}

/// The openssl-made reply with its certificate set listing its one certificate `times` times.
fn certificate_times(times: usize) -> Vec<u8> {
    with_signed_data(&read("local.tsr"), |signed| {
        let set = signed.certificates.as_ref().unwrap();
        assert_eq!(set.0.len(), 1);
        let cert = set.0.get(0).unwrap().to_der().unwrap();
        let mut fields = Vec::new();
        fields.extend(signed.version.to_der().unwrap());
        fields.extend(signed.digest_algorithms.to_der().unwrap());
        fields.extend(signed.encap_content_info.to_der().unwrap());
        fields.extend(tlv(0xa0, &cert.repeat(times)));
        fields.extend(signed.signer_infos.to_der().unwrap());
        fields
    })
}

/// The DER crate refuses a certificate set that lists one certificate twice.
#[test]
fn a_certificate_set_with_a_repeat_is_an_error() {
    assert_eq!(certificate_times(1), read("local.tsr"));
    let e = tsp::parse(&certificate_times(2)).unwrap_err();
    assert!(e.to_string().starts_with("not a time-stamp reply"), "{e}");
}
