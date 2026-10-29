use ignorarium::power::{required_units, z};
use ignorarium::prior::normalized_sha256;

#[test]
fn z_matches_reference_quantiles() {
    // Low tail, middle and high tail of Acklam's three branches.
    for (p, want) in [
        (0.975, 1.959964),
        (0.9975, 2.807034),
        (0.8, 0.841621),
        (0.9, 1.281552),
        (0.01, -2.326348),
        (0.001, -3.090232),
        (0.02, -2.053749),
        (0.99, 2.326348),
        (0.999, 3.090232),
        (0.98, 2.053749),
        (0.3, -0.524401),
    ] {
        assert!((z(p) - want).abs() < 1e-6, "{p}: {}", z(p));
    }
}

#[test]
fn z_is_zero_at_one_half_and_antisymmetric() {
    assert!(z(0.5).abs() < 1e-9);
    for p in [0.001, 0.02, 0.03, 0.2, 0.4] {
        assert!((z(p) + z(1.0 - p)).abs() < 1e-9, "{p}");
    }
}

#[test]
fn z_branches_meet_at_their_joins() {
    // Both sides of 0.02425 and 0.97575 agree to the algorithm's accuracy.
    for p in [0.02425, 0.97575] {
        assert!((z(p - 1e-9) - z(p + 1e-9)).abs() < 1e-6, "{p}");
    }
}

#[test]
fn z_edges_are_infinite_or_nan() {
    assert_eq!(z(0.0), f64::NEG_INFINITY);
    assert_eq!(z(1.0), f64::INFINITY);
    assert_eq!(z(-0.5), f64::NEG_INFINITY);
    assert_eq!(z(1.5), f64::INFINITY);
    assert!(z(f64::NAN).is_nan());
}

#[test]
fn required_units_matches_hand_values() {
    assert_eq!(required_units(0.005, 0.8, 2.0, 1.0, 1, 0.0), 54);
    assert_eq!(required_units(0.005, 0.8, 2.0, 1.0, 10, 0.1), 102);
    assert_eq!(required_units(0.05, 0.8, 1.0, 0.5, 1, 0.0), 32);
}

#[test]
fn required_units_grows_with_icc_and_cluster_size() {
    let base = required_units(0.005, 0.8, 2.0, 1.0, 5, 0.0);
    assert_eq!(base, 54);
    assert_eq!(required_units(0.005, 0.8, 2.0, 1.0, 5, 0.1), 75);
    assert_eq!(required_units(0.005, 0.8, 2.0, 1.0, 1, 0.5), 54);
}

#[test]
fn normalized_hash_known_vector() {
    assert_eq!(
        normalized_sha256("Hello   World\n\tAgain "),
        "cbb2d50bd9c870e0f098b9230d66d2f74d55544f2e57f1aabe3830443a040eb6"
    );
}

#[test]
fn normalized_hash_ignores_case_and_whitespace_only() {
    assert_eq!(
        normalized_sha256("A b"),
        normalized_sha256("  a\u{a0}\u{2003}B\r\n")
    );
    assert_ne!(normalized_sha256("a b"), normalized_sha256("ab"));
    assert_ne!(normalized_sha256("a b"), normalized_sha256("a c"));
}
