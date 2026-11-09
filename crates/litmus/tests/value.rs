use litmus::value;

const N: usize = 10_000;

fn values(condition: &str) -> Vec<f64> {
    (0..N)
        .map(|i| value(format!("c{i}").as_bytes(), 0, condition).unwrap())
        .collect()
}

#[test]
fn ref_is_exactly_zero() {
    assert_eq!(value(b"x", 0, "ref"), Some(0.0));
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

#[test]
fn deterministic() {
    let a = value(b"cluster", 3, "bad").unwrap();
    let b = value(b"cluster", 3, "bad").unwrap();
    assert_eq!(a.to_bits(), b.to_bits());
}
