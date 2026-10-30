use evidence::claim::{Bound, Statistic, e_value, holds, observation, order};
use evidence::rng::Rng;
use evidence::sim::Estimate;
use evidence::{Error, Tail};

const LEVEL: f64 = 0.05;
const REPS: usize = 20_000;

fn below(margin: f64) -> Bound {
    Bound {
        tail: Tail::Lower,
        margin,
    }
}

fn above(margin: f64) -> Bound {
    Bound {
        tail: Tail::Upper,
        margin,
    }
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-12, "{a} != {b}");
}

/// Byte 0 holds the test family, so each test draws its own stream.
fn rng(family: u8) -> Rng {
    let mut s = [7u8; 32];
    s[0] = family;
    Rng::from_seed(s)
}

/// The share of `REPS` replicates of `n` observations from `draw` whose test of `bound`
/// reaches 1 / LEVEL, with its Wilson upper end.
fn rejections(
    stat: Statistic,
    bound: Bound,
    n: usize,
    rng: &mut Rng,
    mut draw: impl FnMut(&mut Rng) -> f64,
) -> Estimate {
    let mut hits = 0.0;
    for _ in 0..REPS {
        let obs: Vec<f64> = (0..n).map(|_| draw(rng)).collect();
        hits += f64::from(u8::from(
            e_value(&stat, &bound, &obs).unwrap() >= 1.0 / LEVEL,
        ));
    }
    Estimate::of(hits, REPS)
}

#[test]
fn a_null_at_the_margin_rejects_at_most_alpha() {
    // Uniform cluster means on (0, 1]: the mean is the margin, the worst case of each null.
    let stat = Statistic::Mean { lo: 0.0, hi: 1.0 };
    let mut r = rng(1);
    for bound in [below(0.5), above(0.5)] {
        let e = rejections(stat, bound, 30, &mut r, Rng::uniform);
        assert!(e.holds(LEVEL), "{bound:?}: {e:?}");
    }
}

#[test]
fn a_skewed_null_still_holds_alpha() {
    // An exponential shifted down by 1 and clipped at 9: values in [-1, 9], skewed right,
    // with mean -e^-10. The test assumes no symmetry, only the declared range.
    let stat = Statistic::Mean { lo: -1.0, hi: 9.0 };
    let margin = -(-10.0f64).exp();
    let skewed = |r: &mut Rng| (-r.uniform().ln()).min(10.0) - 1.0;
    let mut r = rng(2);
    for bound in [below(margin), above(margin)] {
        let e = rejections(stat, bound, 30, &mut r, skewed);
        assert!(e.holds(LEVEL), "{bound:?}: {e:?}");
    }
}

#[test]
fn the_quantile_test_holds_alpha_at_q() {
    // Four uniform units per cluster: a unit lies at or below q with chance q, and at or
    // above q with chance 1 - q, so the margin q is the q-quantile and each null is tight.
    for q in [0.5, 0.9] {
        let stat = Statistic::Quantile(q);
        let mut r = rng(3);
        for bound in [below(q), above(q)] {
            let share = |r: &mut Rng| {
                let units: Vec<f64> = (0..4).map(|_| r.uniform()).collect();
                observation(&stat, &bound, &units).unwrap()
            };
            let e = rejections(stat, bound, 30, &mut r, share);
            assert!(e.holds(LEVEL), "q {q}, {bound:?}: {e:?}");
        }
    }
}

#[test]
fn e_value_follows_the_table() {
    // Two observations: the first bet is 0, the second is S / (1 + Q) capped at 1/2.
    let mean = Statistic::Mean { lo: 0.0, hi: 1.0 };
    // A mean below 0.5: scores -1, -1; the lower tail bets 1/2 and gains 1/2.
    close(e_value(&mean, &below(0.5), &[0.0, 0.0]).unwrap(), 1.5);
    // A mean above 0.5 on the same data never bets.
    close(e_value(&mean, &above(0.5), &[0.0, 0.0]).unwrap(), 1.0);
    close(e_value(&mean, &above(0.5), &[1.0, 1.0]).unwrap(), 1.5);
    // The scale is the longer side of the range: margin 0.25 scores 1 as 1.
    close(e_value(&mean, &above(0.25), &[1.0, 1.0]).unwrap(), 1.5);
    // A quantile bets that the share on the claimed side is high, on both claims.
    // Below: null mean q = 0.25, so a share of 1 scores 1.
    let quarter = Statistic::Quantile(0.25);
    close(e_value(&quarter, &below(9.0), &[1.0, 1.0]).unwrap(), 1.5);
    close(e_value(&quarter, &below(9.0), &[0.0, 0.0]).unwrap(), 1.0);
    // Above: null mean 1 - q = 0.75; a share of 1 scores 1/3, the bet is 0.3.
    close(
        e_value(&quarter, &above(9.0), &[1.0, 1.0]).unwrap(),
        1.0 + 0.3 / 3.0,
    );
    // The order of the observations is the order given.
    let ordered = e_value(&mean, &below(0.5), &[0.0, 1.0]).unwrap();
    let reversed = e_value(&mean, &below(0.5), &[1.0, 0.0]).unwrap();
    close(ordered, 0.5);
    close(reversed, 1.0);
    // No observation: no evidence.
    close(e_value(&mean, &below(0.5), &[]).unwrap(), 1.0);
}

#[test]
fn out_of_range_gives_e_zero() {
    let stat = Statistic::Mean { lo: 0.0, hi: 1.0 };
    for obs in [
        [0.0, -0.001],
        [1.001, 0.0],
        [f64::NAN, 0.0],
        [0.0, f64::INFINITY],
    ] {
        assert_eq!(e_value(&stat, &below(0.5), &obs), Ok(0.0), "{obs:?}");
    }
    // The ends of the range are inside it.
    close(e_value(&stat, &below(0.5), &[0.0, 0.0]).unwrap(), 1.5);
    close(e_value(&stat, &above(0.5), &[1.0, 1.0]).unwrap(), 1.5);
    // A share is always in [0, 1].
    assert_eq!(
        e_value(&Statistic::Quantile(0.5), &below(0.0), &[1.5]),
        Ok(0.0)
    );
}

#[test]
fn a_bad_claim_is_refused() {
    let mean = Statistic::Mean { lo: 0.0, hi: 1.0 };
    let both = Bound {
        tail: Tail::Both,
        margin: 0.5,
    };
    let tail = Err(Error::Parameter("tail"));
    assert_eq!(e_value(&mean, &both, &[0.5]), tail);
    assert_eq!(holds(&mean, &both, &[0.5]), tail.clone().map(|_: f64| true));
    assert_eq!(observation(&mean, &both, &[0.5]), tail);
    let quantile = Statistic::Quantile(0.5);
    assert_eq!(observation(&quantile, &both, &[0.5]), tail);
    // A margin at an end of the range, or a quantile at 0 or 1, cannot be tested.
    let bounds = Err(Error::Parameter("bounds"));
    for (stat, bound) in [
        (mean, below(1.0)),
        (mean, above(0.0)),
        (Statistic::Mean { lo: 1.0, hi: 0.0 }, below(0.5)),
        (Statistic::Quantile(0.0), below(0.0)),
        (Statistic::Quantile(1.0), above(0.0)),
    ] {
        assert_eq!(e_value(&stat, &bound, &[0.5]), bounds, "{stat:?} {bound:?}");
        assert_eq!(
            holds(&stat, &bound, &[0.5]),
            bounds.clone().map(|_: f64| true),
            "{stat:?} {bound:?}"
        );
    }
}

#[test]
fn observation_is_the_mean_or_the_share_on_the_claimed_side() {
    let units = [1.0, 2.0, 2.0, 7.0];
    let mean = Statistic::Mean { lo: 0.0, hi: 10.0 };
    for bound in [below(5.0), above(5.0)] {
        close(observation(&mean, &bound, &units).unwrap(), 3.0);
    }
    // The margin itself is on the claimed side of both bounds.
    let q = Statistic::Quantile(0.5);
    close(observation(&q, &below(2.0), &units).unwrap(), 0.75);
    close(observation(&q, &above(2.0), &units).unwrap(), 0.75);
    close(observation(&q, &below(1.5), &units).unwrap(), 0.25);
    close(observation(&q, &above(1.5), &units).unwrap(), 0.75);
    // A NaN unit is on no side; a cluster with no unit gives no number.
    close(observation(&q, &below(2.0), &[f64::NAN, 1.0]).unwrap(), 0.5);
    assert!(observation(&q, &below(2.0), &[]).unwrap().is_nan());
    assert!(observation(&mean, &below(2.0), &[]).unwrap().is_nan());
}

#[test]
fn holds_is_inclusive_at_the_margin() {
    let mean = Statistic::Mean {
        lo: -10.0,
        hi: 10.0,
    };
    // A mean of exactly 1.
    let at = [0.5, 1.5];
    assert_eq!(holds(&mean, &below(1.0), &at), Ok(true));
    assert_eq!(holds(&mean, &above(1.0), &at), Ok(true));
    assert_eq!(holds(&mean, &below(1.0), &[1.0, 1.5]), Ok(false));
    assert_eq!(holds(&mean, &above(1.0), &[0.5, 1.0]), Ok(false));
    // A quantile claim holds when the mean share reaches q (below) or 1 - q (above).
    let q = Statistic::Quantile(0.75);
    assert_eq!(holds(&q, &below(0.0), &[0.5, 1.0]), Ok(true));
    assert_eq!(holds(&q, &below(0.0), &[0.5, 0.75]), Ok(false));
    assert_eq!(holds(&q, &above(0.0), &[0.0, 0.5]), Ok(true));
    assert_eq!(holds(&q, &above(0.0), &[0.0, 0.25]), Ok(false));
    // No observation holds nothing.
    assert_eq!(holds(&mean, &below(1.0), &[]), Ok(false));
}

#[test]
fn order_is_the_seeded_forward_shuffle() {
    let seed = [3u8; 32];
    let got = order(10, &mut Rng::from_seed(seed));
    let want: Vec<usize> = Rng::from_seed(seed)
        .sample(10, 10)
        .into_iter()
        .map(|i| i as usize)
        .collect();
    assert_eq!(got, want);
    let mut sorted = got.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, (0..10).collect::<Vec<_>>());
    assert_ne!(got, sorted);
    assert!(order(0, &mut Rng::from_seed(seed)).is_empty());
}
