use evidence::Error;
use evidence::tree::decide;

const ALPHA: f64 = 0.0625;
const W: [f64; 3] = [2.0, 1.0, 1.0];

fn close_to(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} vs {b}");
}

#[test]
fn all_ran_splits_by_weight() {
    let v = decide(ALPHA, &W, &[Some(32.0), Some(63.9), Some(64.0)]).unwrap();
    assert_eq!(v.levels, vec![1.0 / 32.0, 1.0 / 64.0, 1.0 / 64.0]);
    assert_eq!(v.rejected, vec![true, false, true]);
    close_to(v.e_value, (2.0 * 32.0 + 63.9 + 64.0) / 4.0, 1e-12);
}

#[test]
fn a_child_that_never_ran_returns_its_share() {
    let v = decide(ALPHA, &W, &[Some(25.0), None, Some(47.0)]).unwrap();
    close_to(v.levels[0], 1.0 / 24.0, 1e-15);
    assert_eq!(v.levels[1], 0.0);
    close_to(v.levels[2], 1.0 / 48.0, 1e-15);
    assert_eq!(v.rejected, vec![true, false, false]);
    let all = decide(ALPHA, &W, &[Some(25.0), Some(1.0), Some(1.0)]).unwrap();
    assert_eq!(all.rejected, vec![false, false, false]);
}

#[test]
fn root_alone_gets_the_whole_level() {
    let v = decide(ALPHA, &W, &[Some(16.0), None, None]).unwrap();
    assert_eq!(v.levels, vec![0.0625, 0.0, 0.0]);
    assert_eq!(v.rejected, vec![true, false, false]);
    assert_eq!(v.e_value, 16.0);
}

#[test]
fn tree_e_value_reaches_one_over_alpha_when_any_is_rejected() {
    let cases = [
        [Some(32.0), Some(63.9), Some(64.0)],
        [Some(25.0), None, Some(47.0)],
        [Some(16.0), None, None],
        [Some(0.0), Some(0.0), Some(64.0)],
        [None, Some(f64::INFINITY), Some(0.0)],
    ];
    let mut rejections = 0;
    for e in cases {
        let v = decide(ALPHA, &W, &e).unwrap();
        if v.rejected.iter().any(|&r| r) {
            rejections += 1;
            assert!(v.e_value >= 1.0 / ALPHA, "{e:?} -> {}", v.e_value);
        }
    }
    assert_eq!(rejections, 5);
}

#[test]
fn zero_level_rejects_nothing() {
    let v = decide(0.0, &W, &[Some(f64::INFINITY), Some(1.0), Some(1.0)]).unwrap();
    assert_eq!(v.levels, vec![0.0; 3]);
    assert_eq!(v.rejected, vec![false; 3]);
}

#[test]
fn infinite_e_value_rejects_at_any_positive_level() {
    let v = decide(1.0, &W, &[Some(f64::INFINITY), None, None]).unwrap();
    assert_eq!(v.rejected, vec![true, false, false]);
    assert_eq!(v.e_value, f64::INFINITY);
}

#[test]
fn nothing_ran() {
    let v = decide(ALPHA, &W, &[None, None, None]).unwrap();
    assert_eq!(v.levels, vec![0.0; 3]);
    assert_eq!(v.rejected, vec![false; 3]);
    assert_eq!(v.e_value, 1.0);
}

#[test]
fn rejects_bad_inputs() {
    let e = [Some(1.0), Some(1.0), Some(1.0)];
    let weights = Error::Parameter("weights");
    for w in [
        [0.0, 1.0, 1.0],
        [-1.0, 1.0, 1.0],
        [f64::NAN, 1.0, 1.0],
        [f64::INFINITY, 1.0, 1.0],
        [f64::MAX, f64::MAX, 1.0],
    ] {
        assert_eq!(decide(ALPHA, &w, &e), Err(weights.clone()), "{w:?}");
    }
    assert_eq!(decide(ALPHA, &[1.0, 1.0], &e), Err(weights.clone()));
    assert_eq!(decide(ALPHA, &W, &e[..2]), Err(weights.clone()));
    assert_eq!(decide(ALPHA, &[], &[]), Err(weights));
    let ev = Error::Parameter("e-value");
    for bad in [f64::NAN, -1.0, f64::NEG_INFINITY] {
        assert_eq!(
            decide(ALPHA, &W, &[Some(1.0), Some(bad), None]),
            Err(ev.clone())
        );
    }
    let alpha = Error::Parameter("alpha");
    for a in [-0.1, 1.5, f64::NAN] {
        assert_eq!(decide(a, &W, &e), Err(alpha.clone()), "{a}");
    }
    assert!(decide(1.0, &W, &e).is_ok());
    assert!(decide(0.0, &W, &e).is_ok());
}

#[test]
fn tiny_weight_share_does_not_make_a_nan_e_value() {
    let v = decide(
        ALPHA,
        &[5e-324, 1e300, 1.0],
        &[Some(f64::INFINITY), Some(1.0), Some(1.0)],
    )
    .unwrap();
    assert_eq!(v.e_value, f64::INFINITY);
}

#[test]
fn a_rejection_always_reaches_one_over_alpha() {
    let alpha = 0.05;
    let level = alpha * 1.0 / 3.0;
    let v = decide(alpha, &[1.0; 3], &[Some(1.0 / level), Some(0.0), Some(0.0)]).unwrap();
    assert!(v.rejected[0]);
    assert!(v.e_value >= 1.0 / 0.05, "{}", v.e_value);

    for alpha in [0.05, 0.005, 0.01, 0.1] {
        for w in [[1.0, 1.0, 1.0], [3.0, 1.0, 1.0], [1.0, 2.0, 7.0]] {
            let total: f64 = w.iter().sum();
            for k in 0..3 {
                let mut e = [Some(0.0); 3];
                e[k] = Some(1.0 / (alpha * w[k] / total));
                let v = decide(alpha, &w, &e).unwrap();
                assert!(v.rejected[k], "{alpha} {w:?} {k}");
                assert!(v.e_value >= 1.0 / alpha, "{alpha} {w:?} {k}: {}", v.e_value);
            }
        }
    }
}
