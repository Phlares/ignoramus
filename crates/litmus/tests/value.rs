use litmus::value;

const N: usize = 10_000;

fn values(condition: &str) -> Vec<f64> {
    (0..N)
        .map(|i| value(format!("c{i}").as_bytes(), 0, condition).unwrap())
        .collect()
}

#[test]
fn ref_is_exactly_zero() {
    assert!(values("ref").iter().all(|v| v.to_bits() == 0));
}

#[test]
fn unknown_condition_is_none() {
    assert_eq!(value(b"x", 0, "nope"), None);
}

#[test]
fn noise_stays_inside_spread() {
    // The literal 0.2 is deliberate: the test must not read `SPREAD`.
    for v in values("good") {
        assert!((v - (-0.6)).abs() <= 0.2, "{v}");
    }
}

#[test]
fn arm_mean_matches() {
    for (condition, mean) in [("good", -0.6), ("bad", 0.3), ("edge", 0.0)] {
        let v = values(condition);
        let got = v.iter().sum::<f64>() / v.len() as f64;
        assert!((got - mean).abs() <= 0.01, "{condition}: {got}");
    }
}

/// Computed outside this crate: u = the first 8 bytes of
/// SHA-256(b"cluster\x00\x00\x00\x00\x03\x00bad") as a big-endian u64 / 2^64,
/// then 0.3 + (u - 0.5) * 2 * 0.2.
#[test]
fn known_answer() {
    assert_eq!(value(b"cluster", 3, "bad"), Some(0.3252671416831579));
}
