use evidence::claim::{Bound, Statistic, clipped, e_value, e_values, holds, observation, order};
use evidence::eprocess::{EProcess, Score};
use evidence::rng::Rng;
use evidence::sim::Estimate;
use evidence::{Error, Tail, rejects};

/// The level a box study runs at, and a looser one.
const LEVELS: [f64; 2] = [0.05, 0.005];
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

/// The e-process the claim test runs for `bound`, built here so a check can read its value
/// after each observation. `path` checks it against `e_values`.
fn process(stat: Statistic, bound: Bound) -> EProcess {
    let (lo, hi, mean, tail) = match stat {
        Statistic::Mean { lo, hi } => (lo, hi, bound.margin, bound.tail),
        Statistic::Quantile(q) if bound.tail == Tail::Lower => (0.0, 1.0, q, Tail::Upper),
        Statistic::Quantile(q) => (0.0, 1.0, 1.0 - q, Tail::Upper),
    };
    EProcess::new(Score::Bounded { lo, hi, mean }, tail, f64::INFINITY).unwrap()
}

/// The test's value before any observation, then after each one.
fn path(stat: Statistic, bound: Bound, obs: &[f64]) -> Vec<f64> {
    let mut p = process(stat, bound);
    let mut out = vec![p.value()];
    for &x in obs {
        p.observe(x).unwrap();
        out.push(p.value());
    }
    let peak = out.iter().copied().fold(0.0, f64::max);
    assert_eq!(e_values(&stat, &bound, obs), Ok((out[obs.len()], peak)));
    out
}

/// The mean of `xs` and its standard error.
fn mean_se(xs: &[f64]) -> (f64, f64) {
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
    (mean, (var / n).sqrt())
}

/// Checks the test of `bound` on a null at its margin, over `REPS` replicates of `n`
/// observations from `draw`. First, `n` copies of `best`, the observation that argues most
/// for the claim, reach 1 / 0.005, so the null can reject at either level. Then:
/// - the share of replicates that reach 1 / level has a Wilson upper end at most the level, at
///   each level;
/// - the mean e-value is at most 1 + 4 standard errors. At the margin it is 1. A heavy right
///   tail pulls a sample mean below 1, not above, so only this side is tested;
/// - each factor e_k / e_(k-1), pooled over every step and replicate, has a mean within 4
///   standard errors of 1, as the steps of a martingale do. Factors lie in [1 - cap, 1 + cap],
///   so their tails are light, and a bet that sees its own observation moves their mean.
fn holds_at_the_margin(
    stat: Statistic,
    bound: Bound,
    n: usize,
    best: f64,
    rng: &mut Rng,
    mut draw: impl FnMut(&mut Rng) -> f64,
) {
    let strongest = e_value(&stat, &bound, &vec![best; n]).unwrap();
    assert!(strongest >= 1.0 / 0.005, "{bound:?}: at most {strongest}");
    let paths: Vec<Vec<f64>> = (0..REPS)
        .map(|_| {
            let obs: Vec<f64> = (0..n).map(|_| draw(rng)).collect();
            path(stat, bound, &obs)
        })
        .collect();
    let es: Vec<f64> = paths.iter().map(|p| p[n]).collect();
    for level in LEVELS {
        let hits = es.iter().filter(|&&e| rejects(e, level)).count();
        let rate = Estimate::of(hits as f64, REPS);
        assert!(rate.holds(level), "{bound:?} at {level}: {rate:?}");
    }
    let (mean, se) = mean_se(&es);
    assert!(mean - 1.0 <= 4.0 * se, "{bound:?}: mean e {mean}, se {se}");
    let factors: Vec<f64> = (paths.iter())
        .flat_map(|p| p.windows(2).map(|w| w[1] / w[0]))
        .collect();
    let (mean, se) = mean_se(&factors);
    assert!(
        (mean - 1.0).abs() <= 4.0 * se,
        "{bound:?}: mean factor {mean}, se {se}"
    );
}

#[test]
fn a_null_at_the_margin_rejects_at_most_alpha() {
    // Uniform cluster means on (0, 1], and the two ends of the range with chance 1/2 each,
    // the null with the most variance: the mean is the margin, the worst case of each null.
    let stat = Statistic::Mean { lo: 0.0, hi: 1.0 };
    let mut r = rng(1);
    let ends = |r: &mut Rng| f64::from(u8::from(r.uniform() <= 0.5));
    for (bound, best) in [(below(0.5), 0.0), (above(0.5), 1.0)] {
        holds_at_the_margin(stat, bound, 30, best, &mut r, Rng::uniform);
        holds_at_the_margin(stat, bound, 30, best, &mut r, ends);
    }
}

#[test]
fn a_skewed_null_still_holds_alpha() {
    // 1 with chance 0.2, else 0: skewed, with mean 0.2 at the margin. The test assumes no
    // symmetry, only the declared range. 60 observations let either claim reach 1/0.005.
    let stat = Statistic::Mean { lo: 0.0, hi: 1.0 };
    let skewed = |r: &mut Rng| f64::from(u8::from(r.uniform() <= 0.2));
    let mut r = rng(2);
    for (bound, best) in [(below(0.2), 0.0), (above(0.2), 1.0)] {
        holds_at_the_margin(stat, bound, 60, best, &mut r, skewed);
    }
}

#[test]
fn the_quantile_test_holds_alpha_at_q() {
    // Four uniform units per cluster: a unit lies at or below q with chance q, and at or
    // above q with chance 1 - q, so the margin q is the q-quantile and each null is tight.
    // Below the 0.9-quantile a share of 1 scores only 1/9, so it takes 102 clusters to reach
    // 1/0.005; the test runs 110.
    for q in [0.5, 0.9] {
        let stat = Statistic::Quantile(q);
        let mut r = rng(3);
        for bound in [below(q), above(q)] {
            let share = |r: &mut Rng| {
                let units: Vec<f64> = (0..4).map(|_| r.uniform()).collect();
                observation(&stat, &bound, &units).unwrap()
            };
            holds_at_the_margin(stat, bound, 110, 1.0, &mut r, share);
        }
    }
}

#[test]
fn clipped_units_hold_alpha_at_the_clipped_mean() {
    // Units uniform on [-0.5, 1.5], clipped to the range [0, 1]: a quarter become 0, a
    // quarter 1, and the clipped mean is 0.5. Every cluster mean lies in the range, so no
    // cluster gives e-value 0, and the clipped mean is what the test holds at its margin.
    let stat = Statistic::Mean { lo: 0.0, hi: 1.0 };
    let mut r = rng(4);
    for (bound, best) in [(below(0.5), 0.0), (above(0.5), 1.0)] {
        let cluster = |r: &mut Rng| {
            let units: Vec<f64> = (0..4).map(|_| 2.0 * r.uniform() - 0.5).collect();
            observation(&stat, &bound, &units).unwrap()
        };
        holds_at_the_margin(stat, bound, 30, best, &mut r, cluster);
    }
}

#[test]
fn e_value_follows_the_table() {
    // Two observations: the first bet is 0, the second is S / (1 + Q), 1/2 after a score of 1.
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
fn e_values_give_the_last_and_the_largest() {
    let mean = Statistic::Mean { lo: 0.0, hi: 1.0 };
    // Scores -1, -1, 1 below 0.5: 1, 1.5, then a bet of 2/3 on the wrong side gives 0.5.
    let (last, peak) = e_values(&mean, &below(0.5), &[0.0, 0.0, 1.0]).unwrap();
    close(last, 0.5);
    close(peak, 1.5);
    // The start counts: a process that never bets peaks at 1.
    let (last, peak) = e_values(&mean, &below(0.5), &[1.0, 1.0, 1.0]).unwrap();
    close(last, 1.0);
    close(peak, 1.0);
    // An observation the test refuses makes both 0.
    assert_eq!(e_values(&mean, &below(0.5), &[0.0, 2.0]), Ok((0.0, 0.0)));
    assert_eq!(
        e_values(&mean, &below(0.5), &[0.0, 0.0]).unwrap().0,
        e_value(&mean, &below(0.5), &[0.0, 0.0]).unwrap()
    );
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
        // Each unit is first clipped to the declared range, the ends included.
        close(
            observation(&mean, &bound, &[-5.0, 20.0, 4.0]).unwrap(),
            14.0 / 3.0,
        );
        close(observation(&mean, &bound, &[0.0, 10.0]).unwrap(), 5.0);
        assert!(
            observation(&mean, &bound, &[f64::NAN, 1.0])
                .unwrap()
                .is_nan()
        );
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
fn a_cluster_at_an_end_of_the_range_stays_inside_it() {
    // Three units at -0.1 sum to -0.30000000000000004, whose third lies just past -0.1, and
    // the same at 0.1. A cluster whose units all lie at an end, or past it, still scores.
    let stat = Statistic::Mean { lo: -0.1, hi: 0.1 };
    for (bound, end) in [(below(0.0), -0.1), (above(0.0), 0.1)] {
        for units in [[end; 3], [50.0 * end; 3]] {
            let x = observation(&stat, &bound, &units).unwrap();
            assert_eq!(x, end, "{units:?}");
            close(e_value(&stat, &bound, &[x, x]).unwrap(), 1.5);
        }
    }
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

#[test]
fn clipped_counts_the_units_below_lo_and_above_hi() {
    let stat = Statistic::Mean { lo: 0.0, hi: 10.0 };
    // A unit at an end is inside the range; NaN lies on no side.
    let units = [-1.0, 0.0, 5.0, 10.0, 11.0, f64::INFINITY, f64::NAN];
    assert_eq!(clipped(&stat, &units), Some((1, 2)));
    assert_eq!(clipped(&stat, &[]), Some((0, 0)));
    let tiny = 0.0_f64.next_down();
    assert_eq!(clipped(&stat, &[tiny, 10.0_f64.next_up()]), Some((1, 1)));
    assert_eq!(clipped(&Statistic::Quantile(0.5), &units), None);
}

#[test]
fn clipped_counts_the_units_observation_moves() {
    let stat = Statistic::Mean { lo: 0.0, hi: 10.0 };
    for x in [-1.0, 0.0, 5.0, 10.0, 11.0] {
        let moved = observation(&stat, &below(5.0), &[x]).unwrap() != x;
        let (lo, hi) = clipped(&stat, &[x]).unwrap();
        assert_eq!(lo + hi == 1, moved, "{x}");
        assert_eq!(lo == 1, x < 0.0, "{x}");
    }
}
