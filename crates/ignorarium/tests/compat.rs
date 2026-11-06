//! Pins what the hash, signature and TOML crates compute. A record the box wrote under one
//! version of them must read, hash and verify the same under the next.
use ed25519_dalek::SigningKey;
use ignorarium::canon::{canonical_line, canonical_of, sha256_hex};
use ignorarium::keys::{sign_hex, verify_hex};
use ignorarium::records::{self, Record};
use ignorarium::spec;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;

fn key(hex_seed: &str) -> SigningKey {
    SigningKey::from_bytes(&hex::decode(hex_seed).unwrap().try_into().unwrap())
}

/// RFC 8032, section 7.1, tests 1 and 2: seed, public key, message, signature.
const RFC_8032: [(&str, &str, &str, &str); 2] = [
    (
        "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
        "",
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
    ),
    (
        "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
        "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
        "72",
        "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
    ),
];

#[test]
fn signatures_match_rfc_8032() {
    for (seed, public, msg, sig) in RFC_8032 {
        let k = key(seed);
        let msg = hex::decode(msg).unwrap();
        assert_eq!(hex::encode(k.verifying_key().to_bytes()), public);
        assert_eq!(sign_hex(&k, &msg), sig);
        assert!(verify_hex(&k.verifying_key(), &msg, sig));
        assert!(!verify_hex(&k.verifying_key(), b"other", sig));
    }
}

#[test]
fn sha256_matches_fips_180_and_streams_the_same() {
    let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    assert_eq!(sha256_hex(b"abc"), abc);
    let mut h = Sha256::new();
    h.update(b"a");
    h.update(b"bc");
    assert_eq!(hex::encode(h.finalize()), abc);
}

/// A stored record line as the box wrote it: seed `07` repeated, a fixed time.
const OLD_LINE: &str = "{\"hash\":\"93b11df00a6996107726b689849dac52ddc53e82270a3c971847a3389280069b\",\"record\":{\"body\":{\"alpha\":0.005,\"n\":3,\"name\":\"ünïcode\"},\"kind\":\"register\",\"prev\":\"0000000000000000000000000000000000000000000000000000000000000000\",\"row\":1,\"seq\":0,\"study\":\"compat\",\"ts\":\"2026-11-06T00:00:00.000Z\"},\"sig\":\"2b7f833c00e99c86d80d9b9335bf2124518f6e11d91fffe5d20bc776fdb78053cee3f7e1394f9f22c70d0c891e431adf57b24975139480ee67e403e860ca240f\"}\n";

fn old_record() -> Record {
    Record {
        seq: 0,
        prev: "0".repeat(64),
        study: "compat".into(),
        row: Some(1),
        kind: "register".into(),
        body: json!({ "alpha": 0.005, "n": 3, "name": "ünïcode" }),
        ts: "2026-11-06T00:00:00.000Z".into(),
    }
}

#[test]
fn a_record_written_before_still_seals_and_verifies() {
    let k = SigningKey::from_bytes(&[7; 32]);
    let record = old_record();
    let bytes = canonical_of(&record).unwrap();
    let stored = json!({
        "record": record,
        "hash": sha256_hex(&bytes),
        "sig": sign_hex(&k, &bytes),
    });
    let line = String::from_utf8(canonical_line(&stored).unwrap()).unwrap();
    assert_eq!(line, OLD_LINE);
    let d = tempfile::tempdir().unwrap();
    let log = d.path().join("records.jsonl");
    std::fs::write(&log, OLD_LINE).unwrap();
    assert_eq!(
        records::read_all(&log, &k.verifying_key()).unwrap(),
        [old_record()]
    );
}

/// Each TOML file of the repo, parsed as a spec: its hash, or the error.
const SPECS: [(&str, &str); 20] = [
    (
        "crates/bundle/tests/fixtures/studies/numeric/citation.toml",
        "sha256:a7c7ba47a7b50f44c89c06bdaeadf0b3561dfef5d8243297720d0b7ae0264f8c",
    ),
    (
        "crates/bundle/tests/fixtures/studies/numeric/spec.toml",
        "sha256:13ef0c7763218fe87345509dadd78dda598125aaf7fda859104a205ce0d39b47",
    ),
    (
        "crates/ignorarium/tests/fixtures/specs/bal-brief.toml",
        "sha256:6ac808390dc15403541d8b5a69f86b186245498704b33ddf84d7b0b45fc6aca8",
    ),
    (
        "crates/ignorarium/tests/fixtures/specs/broken-multibyte-col.toml",
        "TOML error at 1:14: unexpected key or value, expected newline, `#`",
    ),
    (
        "crates/ignorarium/tests/fixtures/specs/broken-multibyte-line.toml",
        "TOML error at 3:9: invalid basic string, expected `\"`",
    ),
    (
        "crates/ignorarium/tests/fixtures/specs/broken-syntax.crlf.toml",
        "TOML error at 3:20: invalid basic string, expected `\"`",
    ),
    (
        "crates/ignorarium/tests/fixtures/specs/broken-syntax.toml",
        "TOML error at 3:20: invalid basic string, expected `\"`",
    ),
    (
        "crates/ignorarium/tests/fixtures/specs/minimal.bom.toml",
        "sha256:2917c85da9c8398a40eed0813965f702e9f171dbc2469a43023edf024033c94b",
    ),
    (
        "crates/ignorarium/tests/fixtures/specs/minimal.crlf.toml",
        "sha256:2917c85da9c8398a40eed0813965f702e9f171dbc2469a43023edf024033c94b",
    ),
    (
        "crates/ignorarium/tests/fixtures/specs/minimal.toml",
        "sha256:2917c85da9c8398a40eed0813965f702e9f171dbc2469a43023edf024033c94b",
    ),
    (
        "crates/ignorarium/tests/fixtures/specs/multiline.crlf.toml",
        "sha256:7d5e7f3e47e984a0154269eb7f0ec77cd005a8aab7273039a2cc0e51002071de",
    ),
    (
        "crates/ignorarium/tests/fixtures/specs/multiline.toml",
        "sha256:7d5e7f3e47e984a0154269eb7f0ec77cd005a8aab7273039a2cc0e51002071de",
    ),
    (
        "crates/ignorarium/tests/fixtures/specs/nonfinite.toml",
        "sha256:fd604548c654feeb55a0662971790a9b9a89d2bf48814668341b074303804dac",
    ),
    (
        "crates/ignorarium/tests/fixtures/specs/resize.toml",
        "sha256:ee0a58dc54cff6454348813ce8f923f767c316d0815e4fe15d3df953604cca8f",
    ),
    (
        "guide/steps.toml",
        "sha256:ea24ccf60ec734ba5c85f8570162dad7dbf4fa32ea83cb2337c64ecc4ff662ae",
    ),
    (
        "rules/publish.toml",
        "sha256:7bb608af6ed32bb0982f2a177b43d7bd95df0320b1a9f02f261472fd174ee572",
    ),
    (
        "rules/scan.toml",
        "sha256:66d9c7f7bccbc8c78a28424784cf8c4fc6170154c1264f6b3580bf2a5db3d0bd",
    ),
    (
        "rules/v0.toml",
        "sha256:eb966ab4266b65cc9b194e3f164b3199b66b948ee5dc6ce108cf44c1978c8892",
    ),
    (
        "templates/numeric-ablation.toml",
        "sha256:277ee1e16fd66fecfda95c67a05b28523f720e5870cdc09f75844c9c8108ed7e",
    ),
    (
        "templates/planted-spawn.toml",
        "sha256:0c576f2158aa484066174edf30ccc310bbe6fd82b3407daa3b9326bca9fc2e4b",
    ),
];

#[test]
fn every_toml_file_parses_as_before() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut got = Vec::new();
    for (path, want) in SPECS {
        let seen = match spec::load(&root.join(path)) {
            Ok(s) => s.hash,
            Err(e) => e.to_string(),
        };
        if seen != want {
            got.push(format!("{path}: {seen}"));
        }
    }
    assert!(got.is_empty(), "{}", got.join("\n"));
}
