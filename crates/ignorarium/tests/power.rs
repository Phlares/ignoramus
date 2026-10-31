use ignorarium::power::{required_clusters, z};
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
fn required_clusters_matches_the_one_sided_formula() {
    // ceil(((2.5758 + 0.8416) x 2.76 / 2.0)^2) = ceil(22.24).
    assert_eq!(required_clusters(0.005, 0.8, 2.76, 2.0), 23);
    // One-sided: alpha is not halved. A two-sided test would need 54 and 32 here.
    assert_eq!(required_clusters(0.005, 0.8, 2.0, 1.0), 47);
    assert_eq!(required_clusters(0.05, 0.8, 1.0, 0.5), 25);
    // Half the distance needs about four times the clusters; more power or less alpha, more.
    assert_eq!(required_clusters(0.05, 0.8, 1.0, 0.25), 99);
    assert_eq!(required_clusters(0.005, 0.9, 2.0, 1.0), 60);
    assert_eq!(required_clusters(0.0025, 0.8, 2.0, 1.0), 54);
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

#[test]
fn no_prior_use_finds_nothing_and_prior_errors_print_their_text() {
    use ignorarium::prior::{NoPriorUse, PriorError, PriorUse};
    assert_eq!(NoPriorUse.used("a", "b"), Ok(None));
    assert_eq!(PriorError("log is torn".into()).to_string(), "log is torn");
}
