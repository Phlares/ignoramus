use ed25519_dalek::{SigningKey, VerifyingKey};
use ignorarium::planlog::merkle::{
    consistency_proof, inclusion_proof, leaf_hash, node_hash, root, verify_consistency,
    verify_inclusion,
};
use ignorarium::planlog::store::{Leaf, LogError, PlanLog, Verified};
use ignorarium::planlog::{commitment, family};
use ignorarium::prior::PriorUse;
use ignorarium::spec::model::Spec;
use std::fs;
use std::path::Path;

const LEAVES: [&str; 8] = [
    "",
    "00",
    "10",
    "2021",
    "3031",
    "40414243",
    "5051525354555657",
    "606162636465666768696a6b6c6d6e6f",
];
const ROOTS: [&str; 9] = [
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    "6e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d",
    "fac54203e7cc696cf0dfcb42c92a1d9dbaf70ad9e621f4bd8d98662f00e3c125",
    "aeb6bcfe274b70a14fb067a5e5578264db0fa9b51af5e0ba159158f329e06e77",
    "d37ee418976dd95753c1c73862b9398fa2a2cf9b4ff0fdfe8b30cd95209614b7",
    "4e3bbb1f7b478dcfe71fb631631519a3bca12c9aefca1612bfce4c13a86264d4",
    "76e67dadbcdf1e10e1b74ddc608abd2f98dfb16fbce75277b5232a127f2087ef",
    "ddb89be403809e325750d3d263cd78929c2942b7942a34b77e122c9594a74c8c",
    "5dc9da79a70659a9ad559cb701ded9a2ab9d823aad2f4960cfe370eff4604328",
];
const VECTORS: &str = "\
incl 0 1 []
incl 0 8 ['96a296d224f285c67bee93c30f8a309157f0daa35dc5b87e410b78630a09cfc7', '5f083f0a1a33ca076a95279832580db3e0ef4584bdff1f54c8a360f50de3031e', '6b47aaf29ee3c2af9af889bc1fb9254dabd31177f16232dd6aab035ca39bf6e4']
incl 5 8 ['bc1a0643b12e4d2d7c77918f44e0f4f79a838b6cf9ec5b5c283e1f4d88599e6b', 'ca854ea128ed050b41b35ffc1b87b8eb2bde461e9e3b5596ece6b9d5975a0ae0', 'd37ee418976dd95753c1c73862b9398fa2a2cf9b4ff0fdfe8b30cd95209614b7']
incl 2 3 ['fac54203e7cc696cf0dfcb42c92a1d9dbaf70ad9e621f4bd8d98662f00e3c125']
incl 1 5 ['6e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d', '5f083f0a1a33ca076a95279832580db3e0ef4584bdff1f54c8a360f50de3031e', 'bc1a0643b12e4d2d7c77918f44e0f4f79a838b6cf9ec5b5c283e1f4d88599e6b']
cons 1 1 []
cons 1 8 ['96a296d224f285c67bee93c30f8a309157f0daa35dc5b87e410b78630a09cfc7', '5f083f0a1a33ca076a95279832580db3e0ef4584bdff1f54c8a360f50de3031e', '6b47aaf29ee3c2af9af889bc1fb9254dabd31177f16232dd6aab035ca39bf6e4']
cons 6 8 ['0ebc5d3437fbe2db158b9f126a1d118e308181031d0a949f8dededebc558ef6a', 'ca854ea128ed050b41b35ffc1b87b8eb2bde461e9e3b5596ece6b9d5975a0ae0', 'd37ee418976dd95753c1c73862b9398fa2a2cf9b4ff0fdfe8b30cd95209614b7']
cons 2 5 ['5f083f0a1a33ca076a95279832580db3e0ef4584bdff1f54c8a360f50de3031e', 'bc1a0643b12e4d2d7c77918f44e0f4f79a838b6cf9ec5b5c283e1f4d88599e6b']
";

fn ok(n: u64) -> Verified {
    Verified {
        signed: n,
        leaves: n,
    }
}

fn data() -> Vec<Vec<u8>> {
    LEAVES.iter().map(|h| hex::decode(h).unwrap()).collect()
}

fn h32(s: &str) -> [u8; 32] {
    hex::decode(s).unwrap().try_into().unwrap()
}

fn root_of(n: usize) -> [u8; 32] {
    root(&data()[..n])
}

/// (kind, a, b, proof) per vector line.
fn vectors() -> Vec<(&'static str, usize, usize, Vec<[u8; 32]>)> {
    VECTORS
        .lines()
        .map(|l| {
            let (head, rest) = l.split_once('[').unwrap();
            let mut it = head.split_whitespace();
            let kind = it.next().unwrap();
            let a = it.next().unwrap().parse().unwrap();
            let b = it.next().unwrap().parse().unwrap();
            let proof = rest.split('\'').skip(1).step_by(2).map(h32).collect();
            (kind, a, b, proof)
        })
        .collect()
}

#[test]
fn roots_match_rfc_vectors() {
    for (n, want) in ROOTS.iter().enumerate() {
        assert_eq!(hex::encode(root_of(n)), *want, "n = {n}");
    }
}

#[test]
fn hashes_use_domain_prefixes() {
    let l = leaf_hash(b"abc");
    assert_eq!(
        hex::encode(l),
        "609f6e36d2405585188d5cfd761f407c7cc46a7d3f314c88270469dde315fcd1"
    );
    assert_eq!(
        hex::encode(node_hash(&l, &l)),
        "2affb1ee66535319d17552a1d471be7c6b88b6e0ec4d2764beb6f515ae31de7c"
    );
}

#[test]
fn rfc_vectors_verify_and_match_generated_proofs() {
    let d = data();
    for (kind, a, b, proof) in vectors() {
        if kind == "incl" {
            assert_eq!(inclusion_proof(&d[..b], a), proof, "{kind} {a} {b}");
            assert!(verify_inclusion(
                &d[a],
                a as u64,
                b as u64,
                &proof,
                &root_of(b)
            ));
        } else {
            assert_eq!(consistency_proof(&d[..b], a), proof, "{kind} {a} {b}");
            assert!(verify_consistency(
                a as u64,
                b as u64,
                &root_of(a),
                &root_of(b),
                &proof
            ));
        }
    }
}

fn flip(h: &[u8; 32], bit: usize) -> [u8; 32] {
    let mut o = *h;
    o[bit / 8] ^= 1 << (bit % 8);
    o
}

#[test]
fn every_inclusion_and_consistency_proof_verifies_and_a_flipped_bit_fails() {
    let d = data();
    for n in 1..=8usize {
        let rt = root_of(n);
        for i in 0..n {
            let p = inclusion_proof(&d[..n], i);
            let (ii, nn) = (i as u64, n as u64);
            assert!(verify_inclusion(&d[i], ii, nn, &p, &rt), "incl {i}/{n}");
            for k in 0..p.len() {
                for bit in 0..256 {
                    let mut q = p.clone();
                    q[k] = flip(&q[k], bit);
                    assert!(!verify_inclusion(&d[i], ii, nn, &q, &rt));
                }
            }
            for bit in 0..256 {
                assert!(!verify_inclusion(&d[i], ii, nn, &p, &flip(&rt, bit)));
            }
            for j in (0..n).filter(|&j| j != i) {
                assert!(!verify_inclusion(&d[i], j as u64, nn, &p, &rt));
            }
            // A claimed size with a different proof length must fail. Equal
            // lengths can pass: an unchanged subtree hash binds no size.
            for m in (1..=8usize).filter(|&m| m != n) {
                if m <= i || inclusion_proof(&d[..m], i).len() != p.len() {
                    assert!(
                        !verify_inclusion(&d[i], ii, m as u64, &p, &rt),
                        "size {m} for {i}/{n}"
                    );
                }
            }
            assert!(!verify_inclusion(b"other", ii, nn, &p, &rt));
        }
        for m in 1..=n {
            let p = consistency_proof(&d[..n], m);
            let (mm, nn) = (m as u64, n as u64);
            let old = root_of(m);
            assert!(verify_consistency(mm, nn, &old, &rt, &p), "cons {m}/{n}");
            for k in 0..p.len() {
                for bit in 0..256 {
                    let mut q = p.clone();
                    q[k] = flip(&q[k], bit);
                    assert!(!verify_consistency(mm, nn, &old, &rt, &q));
                }
            }
            for bit in 0..256 {
                assert!(!verify_consistency(mm, nn, &flip(&old, bit), &rt, &p));
                assert!(!verify_consistency(mm, nn, &old, &flip(&rt, bit), &p));
            }
            for w in (1..=8usize).filter(|&w| w != m) {
                if w > n || consistency_proof(&d[..n], w).len() != p.len() {
                    assert!(
                        !verify_consistency(w as u64, nn, &old, &rt, &p),
                        "old {w} for {m}/{n}"
                    );
                }
            }
            for w in (1..=8usize).filter(|&w| w != n) {
                if w < m || consistency_proof(&d[..w], m).len() != p.len() {
                    assert!(
                        !verify_consistency(mm, w as u64, &old, &rt, &p),
                        "new {w} for {m}/{n}"
                    );
                }
            }
        }
    }
}

#[test]
fn verification_never_panics_on_malformed_input() {
    let d = data();
    let rt = root_of(8);
    let p = inclusion_proof(&d, 3);
    assert!(!verify_inclusion(&d[3], 8, 8, &p, &rt)); // index == size
    assert!(!verify_inclusion(&d[3], 9, 8, &p, &rt)); // index > size
    assert!(!verify_inclusion(&d[3], 0, 0, &[], &rt)); // size 0
    assert!(!verify_inclusion(&d[3], 3, 8, &p[..2], &rt)); // short proof
    let mut long = p.clone();
    long.push([0; 32]);
    assert!(!verify_inclusion(&d[3], 3, 8, &long, &rt)); // long proof
    assert!(!verify_inclusion(&d[3], 3, 8, &[], &rt));
    assert!(!verify_inclusion(&d[3], u64::MAX, u64::MAX, &p, &rt));

    let c = consistency_proof(&d, 3);
    let r3 = root_of(3);
    assert!(!verify_consistency(8, 3, &rt, &r3, &c)); // old > new
    assert!(!verify_consistency(0, 8, &root_of(0), &rt, &c)); // old size 0
    assert!(!verify_consistency(0, 0, &root_of(0), &root_of(0), &[]));
    assert!(!verify_consistency(3, 8, &r3, &rt, &c[..c.len() - 1]));
    let mut long = c.clone();
    long.push([0; 32]);
    assert!(!verify_consistency(3, 8, &r3, &rt, &long));
    assert!(!verify_consistency(3, 8, &r3, &rt, &[]));
    assert!(!verify_consistency(4, 8, &root_of(4), &rt, &[])); // power of two, empty proof
    assert!(!verify_consistency(3, 3, &r3, &r3, &c)); // equal sizes need an empty proof
    assert!(!verify_consistency(3, 3, &r3, &rt, &[])); // equal sizes, roots differ
    assert!(verify_consistency(3, 3, &r3, &r3, &[]));
    assert!(!verify_consistency(u64::MAX, u64::MAX - 1, &r3, &rt, &c));
    assert!(!verify_consistency(1, u64::MAX, &root_of(1), &rt, &c));
}

#[test]
fn proof_generators_return_empty_on_bad_arguments() {
    let d = data();
    assert!(inclusion_proof(&d, 8).is_empty());
    assert!(inclusion_proof(&[], 0).is_empty());
    assert!(consistency_proof(&d, 0).is_empty());
    assert!(consistency_proof(&d, 9).is_empty());
}

// ---- store ----

fn key(b: u8) -> SigningKey {
    SigningKey::from_bytes(&[b; 32])
}

fn vk(b: u8) -> VerifyingKey {
    key(b).verifying_key()
}

fn leaf(kind: &str, n: u32) -> Leaf {
    Leaf {
        kind: kind.into(),
        commitment: Some(format!("c{n}")),
        family: None,
        mode: None,
        data_hashes: vec![(format!("exact{n}"), format!("norm{n}"))],
        label: format!("leaf {n}"),
        ts: format!("2026-10-28T00:00:{n:02}.000Z"),
    }
}

fn log_with(dir: &Path, n: u32) -> PlanLog {
    let log = PlanLog::open(dir).unwrap();
    for i in 0..n {
        log.append(leaf("plan", i), &key(1)).unwrap();
    }
    log
}

fn leaf_lines(dir: &Path) -> Vec<Vec<u8>> {
    fs::read(dir.join("leaves.jsonl"))
        .unwrap()
        .split_inclusive(|&b| b == b'\n')
        .map(<[u8]>::to_vec)
        .collect()
}

#[test]
fn append_returns_index_and_head_and_verify_counts_leaves() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("pl");
    let log = PlanLog::open(&dir).unwrap();
    assert_eq!(log.verify(&vk(1)).unwrap(), ok(0));
    for i in 0..3u32 {
        let (idx, head) = log.append(leaf("plan", i), &key(1)).unwrap();
        assert_eq!(idx, u64::from(i));
        assert_eq!(head.size, u64::from(i) + 1);
        let raw: Vec<Vec<u8>> = leaf_lines(&dir)
            .iter()
            .map(|l| l[..l.len() - 1].to_vec())
            .collect();
        assert_eq!(head.root, hex::encode(root(&raw)));
    }
    assert_eq!(log.verify(&vk(1)).unwrap(), ok(3));
    assert_eq!(log.heads().unwrap().len(), 3);
    assert!(dir.join("lock").exists());
    assert!(matches!(
        log.verify(&vk(2)),
        Err(LogError::Head { index: 0, .. })
    ));
}

#[test]
fn leaf_line_is_canonical_json_of_the_leaf() {
    let t = tempfile::tempdir().unwrap();
    log_with(t.path(), 1);
    let line = String::from_utf8(leaf_lines(t.path())[0].clone()).unwrap();
    assert_eq!(
        line,
        "{\"commitment\":\"c0\",\"data_hashes\":[[\"exact0\",\"norm0\"]],\"family\":null,\
\"kind\":\"plan\",\"label\":\"leaf 0\",\"mode\":null,\"ts\":\"2026-10-28T00:00:00.000Z\"}\n"
    );
}

#[cfg(unix)]
#[test]
fn open_creates_a_private_dir_and_refuses_a_symlink() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("pl");
    PlanLog::open(&dir).unwrap();
    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let link = t.path().join("ln");
    std::os::unix::fs::symlink(&dir, &link).unwrap();
    assert!(PlanLog::open(&link).is_err());
}

#[test]
fn tampered_leaf_names_the_first_bad_head() {
    let t = tempfile::tempdir().unwrap();
    let log = log_with(t.path(), 4);
    let mut lines = leaf_lines(t.path());
    lines[2][12] ^= 1; // leaf 2 first matters to head 2 (size 3)
    fs::write(t.path().join("leaves.jsonl"), lines.concat()).unwrap();
    match log.verify(&vk(1)) {
        Err(e @ LogError::Head { index: 2, .. }) => assert!(e.to_string().contains("head 2")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn tampered_head_signature_and_order_fail() {
    let t = tempfile::tempdir().unwrap();
    let log = log_with(t.path(), 3);
    let path = t.path().join("heads.jsonl");
    let orig = fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = orig.lines().collect();
    let sig = log.heads().unwrap()[1].sig.clone();
    let first = if sig.starts_with('0') { "1" } else { "0" };
    let bad_sig = format!("{first}{}", &sig[1..]);
    let edited = lines[1].replace(&sig, &bad_sig);
    assert_ne!(edited, lines[1]);
    let write = |a: &str, b: &str, c: &str| fs::write(&path, format!("{a}\n{b}\n{c}\n")).unwrap();
    write(lines[0], &edited, lines[2]);
    assert!(matches!(
        log.verify(&vk(1)),
        Err(LogError::Head { index: 1, .. })
    ));
    // swapped: sizes no longer increase
    write(lines[1], lines[0], lines[2]);
    assert!(matches!(
        log.verify(&vk(1)),
        Err(LogError::Head { index: 1, .. })
    ));
    // repeated head: not strictly increasing
    write(lines[0], lines[1], lines[1]);
    assert!(matches!(
        log.verify(&vk(1)),
        Err(LogError::Head { index: 2, .. })
    ));
    // a line that is not a head
    write(lines[0], "not json", lines[2]);
    assert!(matches!(
        log.verify(&vk(1)),
        Err(LogError::Head { index: 1, .. })
    ));
    // restored
    write(lines[0], lines[1], lines[2]);
    assert_eq!(log.verify(&vk(1)).unwrap(), ok(3));
}

#[test]
fn tampered_head_root_fails() {
    let t = tempfile::tempdir().unwrap();
    let log = log_with(t.path(), 2);
    let path = t.path().join("heads.jsonl");
    let root0 = log.heads().unwrap()[0].root.clone();
    let orig = fs::read_to_string(&path).unwrap();
    let bad = format!(
        "{}{}",
        if root0.starts_with('0') { "1" } else { "0" },
        &root0[1..]
    );
    fs::write(&path, orig.replacen(&root0, &bad, 1)).unwrap();
    assert!(matches!(
        log.verify(&vk(1)),
        Err(LogError::Head { index: 0, .. })
    ));
}

#[test]
fn deleted_leaf_line_is_an_error() {
    let t = tempfile::tempdir().unwrap();
    let log = log_with(t.path(), 3);
    let lines = leaf_lines(t.path());
    fs::write(t.path().join("leaves.jsonl"), lines[..2].concat()).unwrap();
    assert!(matches!(
        log.verify(&vk(1)),
        Err(LogError::Head { index: 2, .. })
    ));
    fs::write(t.path().join("leaves.jsonl"), lines[1..].concat()).unwrap();
    assert!(matches!(
        log.verify(&vk(1)),
        Err(LogError::Head { index: 0, .. })
    ));
}

#[test]
fn a_leaf_without_a_head_is_tolerated_and_next_append_covers_it() {
    let t = tempfile::tempdir().unwrap();
    let log = log_with(t.path(), 2);
    let mut lines = leaf_lines(t.path());
    lines.push(b"{\"orphan\":1}\n".to_vec());
    fs::write(t.path().join("leaves.jsonl"), lines.concat()).unwrap();
    assert_eq!(
        log.verify(&vk(1)).unwrap(),
        Verified {
            signed: 2,
            leaves: 3
        }
    );
    let (idx, head) = log.append(leaf("plan", 9), &key(1)).unwrap();
    assert_eq!((idx, head.size), (3, 4));
    assert_eq!(log.verify(&vk(1)).unwrap(), ok(4));
}

#[test]
fn a_partial_last_line_in_either_file_refuses_and_writes_nothing() {
    for victim in ["leaves.jsonl", "heads.jsonl"] {
        let t = tempfile::tempdir().unwrap();
        let log = log_with(t.path(), 2);
        let path = t.path().join(victim);
        let mut b = fs::read(&path).unwrap();
        b.extend_from_slice(b"{\"par");
        fs::write(&path, b).unwrap();
        let before = (
            fs::read(t.path().join("leaves.jsonl")).unwrap(),
            fs::read(t.path().join("heads.jsonl")).unwrap(),
        );
        assert!(
            matches!(
                log.append(leaf("plan", 5), &key(1)),
                Err(LogError::CorruptTail)
            ),
            "{victim}"
        );
        let after = (
            fs::read(t.path().join("leaves.jsonl")).unwrap(),
            fs::read(t.path().join("heads.jsonl")).unwrap(),
        );
        assert_eq!(before, after, "{victim}");
        assert!(matches!(log.verify(&vk(1)), Err(LogError::CorruptTail)));
    }
}

#[test]
fn prior_use_fails_closed_on_a_corrupt_or_unreadable_log() {
    let t = tempfile::tempdir().unwrap();
    let log = log_with(t.path(), 2);
    let path = t.path().join("leaves.jsonl");
    let good = fs::read(&path).unwrap();
    // partial tail
    let mut b = good.clone();
    b.extend_from_slice(b"{\"par");
    fs::write(&path, b).unwrap();
    assert!(log.used("exact0", "x").is_err());
    // an unparsable line is an error, not skipped, even when a later leaf would match
    let lines: Vec<&[u8]> = good.split_inclusive(|&b| b == b'\n').collect();
    fs::write(
        &path,
        [
            &b"nonsense
"[..],
            lines[1],
        ]
        .concat(),
    )
    .unwrap();
    let e = log.used("exact1", "x").unwrap_err();
    assert!(e.to_string().contains("leaf 0"), "{e}");
    // a directory where the file should be is unreadable
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(log.used("exact0", "x").is_err());
}

#[test]
fn unknown_leaf_fields_are_tolerated() {
    let t = tempfile::tempdir().unwrap();
    let log = PlanLog::open(t.path()).unwrap();
    let line = "{\"commitment\":null,\"data_hashes\":[[\"e1\",\"n1\"]],\"family\":null,\"future\":7,\"kind\":\"plan\",\"label\":\"l\",\"mode\":null,\"ts\":\"t\"}
";
    fs::write(t.path().join("leaves.jsonl"), line).unwrap();
    assert_eq!(log.used("e1", "").unwrap(), Some("leaf 0 (plan)".into()));
}

#[cfg(unix)]
#[test]
fn readers_and_writers_refuse_symlinked_log_files() {
    let t = tempfile::tempdir().unwrap();
    let log = log_with(t.path(), 1);
    for name in ["leaves.jsonl", "heads.jsonl"] {
        let p = t.path().join(name);
        let real = t.path().join(format!("{name}.real"));
        fs::rename(&p, &real).unwrap();
        std::os::unix::fs::symlink(&real, &p).unwrap();
        assert!(log.verify(&vk(1)).is_err(), "{name}");
        assert_eq!(log.heads().is_err(), name == "heads.jsonl", "{name}");
        assert_eq!(
            log.used("a", "b").is_err(),
            name == "leaves.jsonl",
            "{name}"
        );
        assert!(log.append(leaf("plan", 7), &key(1)).is_err(), "{name}");
        fs::remove_file(&p).unwrap();
        fs::rename(&real, &p).unwrap();
    }
    assert_eq!(log.verify(&vk(1)).unwrap(), ok(1));
}

#[test]
fn concurrent_appends_all_land() {
    let t = tempfile::tempdir().unwrap();
    let log = PlanLog::open(t.path()).unwrap();
    std::thread::scope(|s| {
        for w in 0..2u32 {
            let log = &log;
            s.spawn(move || {
                for i in 0..10 {
                    log.append(leaf("plan", w * 20 + i), &key(1)).unwrap();
                }
            });
        }
    });
    assert_eq!(log.verify(&vk(1)).unwrap(), ok(20));
    let sizes: Vec<u64> = log.heads().unwrap().iter().map(|h| h.size).collect();
    assert_eq!(sizes, (1..=20).collect::<Vec<u64>>());
}

#[test]
fn prior_use_sees_legacy_and_plan_leaves_by_exact_or_normalized() {
    let t = tempfile::tempdir().unwrap();
    let log = PlanLog::open(t.path()).unwrap();
    let mut legacy = leaf("legacy", 0);
    legacy.data_hashes = vec![("ex-a".into(), "no-a".into())];
    let mut plan = leaf("plan", 1);
    plan.data_hashes = vec![
        ("ex-b".into(), "no-b".into()),
        ("ex-c".into(), "no-c".into()),
    ];
    log.append(legacy, &key(1)).unwrap();
    log.append(plan, &key(1)).unwrap();
    let u = |e: &str, n: &str| log.used(e, n).unwrap();
    assert_eq!(u("ex-a", "zzz"), Some("leaf 0 (legacy)".into()));
    assert_eq!(u("zzz", "no-a"), Some("leaf 0 (legacy)".into()));
    assert_eq!(u("ex-c", "zzz"), Some("leaf 1 (plan)".into()));
    assert_eq!(u("zzz", "no-b"), Some("leaf 1 (plan)".into()));
    assert_eq!(u("ex-x", "no-x"), None);
    assert_eq!(u("no-a", "ex-a"), None);
}

#[test]
fn prior_use_ignores_an_empty_normalized_hash() {
    let t = tempfile::tempdir().unwrap();
    let log = PlanLog::open(t.path()).unwrap();
    let mut binary = leaf("legacy", 0);
    binary.data_hashes = vec![("ex-bin".into(), String::new())];
    log.append(binary, &key(1)).unwrap();
    assert_eq!(log.used("other", "").unwrap(), None);
    assert_eq!(
        log.used("ex-bin", "").unwrap(),
        Some("leaf 0 (legacy)".into())
    );
}

#[test]
fn prior_use_on_an_empty_log_is_none() {
    let t = tempfile::tempdir().unwrap();
    assert_eq!(
        PlanLog::open(t.path()).unwrap().used("a", "b").unwrap(),
        None
    );
}

// ---- family and commitment ----

fn template() -> Spec {
    let raw = ignorarium::spec::load("../../templates/numeric-ablation.toml".as_ref()).unwrap();
    Spec::from_value(&raw.value).unwrap()
}

#[test]
fn family_is_sixteen_hex_chars_and_stable() {
    let f = family(&template());
    assert_eq!(f.len(), 16);
    assert!(
        f.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert_eq!(f, family(&template()));
}

#[test]
fn family_ignores_order_and_tracks_ids_units_and_payload() {
    let base = template();
    let f = family(&base);
    let mut two = base.clone();
    let mut o2 = two.outcomes[0].clone();
    o2.id = format!("{}-zz", o2.id);
    two.outcomes.push(o2);
    let mut h2 = two.question.hypotheses[0].clone();
    h2.id = format!("{}-zz", h2.id);
    two.question.hypotheses.push(h2);
    let f2 = family(&two);
    assert_ne!(f, f2);
    let mut rev = two.clone();
    rev.outcomes.reverse();
    rev.question.hypotheses.reverse();
    assert_eq!(family(&rev), f2);

    let mut u = base.clone();
    u.outcomes[0].unit.push('x');
    assert_ne!(family(&u), f);
    let mut i = base.clone();
    i.outcomes[0].id.push('x');
    assert_ne!(family(&i), f);
    let mut h = base.clone();
    h.question.hypotheses[0].id.push('x');
    assert_ne!(family(&h), f);
    let mut p = base.clone();
    p.pins.payload.push('x');
    assert_ne!(family(&p), f);
    let mut q = base.clone();
    q.question.population.push('x');
    assert_eq!(family(&q), f);
}

#[test]
fn commitment_is_sha256_of_salt_then_spec() {
    assert_eq!(
        commitment(&[0u8; 32], b"{}"),
        "fb1634c4e6cd4b1f1afe3ce2d6ef080f7ac5d1f357d723ad4dfca782733b62db"
    );
    assert_ne!(commitment(&[1u8; 32], b"{}"), commitment(&[0u8; 32], b"{}"));
    assert_ne!(
        commitment(&[0u8; 32], b"{ }"),
        commitment(&[0u8; 32], b"{}")
    );
}
