//! The claim model. A claim is one or two one-sided bounds on a contrast of paired values; it
//! holds when every bound holds (intersection-union: Berger, Technometrics 24(4), 1982,
//! doi:10.1080/00401706.1982.10487790). Each cluster gives the test one observation of a
//! statistic.
use crate::eprocess::{EProcess, Score};
use crate::rng::Rng;
use crate::{Error, Tail};

/// One one-sided bound. `tail` is the claimed side: `Lower` claims the value lies below
/// `margin`, `Upper` above it. It is not the side the e-process bets on: a quantile claim's
/// test bets on a high share of units on the claimed side, for either tail. `Both` names no
/// side, and every function here refuses it with `Error::Parameter("tail")`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bound {
    pub tail: Tail,
    pub margin: f64,
}

/// What each cluster gives the test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Statistic {
    /// The mean of the cluster's units, each first clipped to `[lo, hi]`, so the mean lies in
    /// the range too.
    Mean { lo: f64, hi: f64 },
    /// The share of the cluster's units on the claimed side of the margin; for the quantile
    /// `q` (0.5 is the median).
    Quantile(f64),
}

/// `x` lies on `tail`'s side of `point`, `point` included. NaN lies on no side.
fn on_side(x: f64, point: f64, tail: Tail) -> bool {
    match tail {
        Tail::Upper => x >= point,
        Tail::Lower => x <= point,
        Tail::Both => false,
    }
}

fn claimed_side(bound: &Bound) -> Result<Tail, Error> {
    match bound.tail {
        Tail::Both => Err(Error::Parameter("tail")),
        side => Ok(side),
    }
}

/// The test of one bound: a fixed-size e-process, and the value the mean of its observations
/// must reach on the side it bets on. A mean bets on the claimed side of the margin. A quantile
/// bets that the share on the claimed side exceeds q (below) or 1 - q (above).
fn test(stat: &Statistic, bound: &Bound) -> Result<(EProcess, f64, Tail), Error> {
    let (lo, hi, point, tail) = match (*stat, claimed_side(bound)?) {
        (Statistic::Mean { lo, hi }, side) => (lo, hi, bound.margin, side),
        (Statistic::Quantile(q), Tail::Lower) => (0.0, 1.0, q, Tail::Upper),
        (Statistic::Quantile(q), _) => (0.0, 1.0, 1.0 - q, Tail::Upper),
    };
    let score = Score::Bounded {
        lo,
        hi,
        mean: point,
    };
    Ok((EProcess::new(score, tail, f64::INFINITY)?, point, tail))
}

fn mean(xs: impl ExactSizeIterator<Item = f64>) -> f64 {
    let n = xs.len() as f64;
    xs.sum::<f64>() / n
}

/// `x` moved to the nearer end of `[lo, hi]` when it lies outside; NaN stays NaN.
fn clip(x: f64, lo: f64, hi: f64) -> f64 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

/// What one cluster gives the test of `bound`: the mean of its units, each clipped to the
/// declared range first, or the share of them on the claimed side (at or below the margin for
/// `Lower`, at or above it for `Upper`). A cluster with no unit, or with a NaN unit in a mean,
/// gives NaN, which the test scores as e-value 0.
pub fn observation(stat: &Statistic, bound: &Bound, units: &[f64]) -> Result<f64, Error> {
    let side = claimed_side(bound)?;
    Ok(match *stat {
        // The mean is clipped too: rounding can carry the mean of units at an end past it.
        Statistic::Mean { lo, hi } => clip(mean(units.iter().map(|&x| clip(x, lo, hi))), lo, hi),
        Statistic::Quantile(_) => {
            let on = units.iter().filter(|&&x| on_side(x, bound.margin, side));
            on.count() as f64 / units.len() as f64
        }
    })
}

/// The e-value of `bound` over `obs`, one per cluster, in the order given. An observation the
/// e-process refuses (outside the declared range, or not finite) makes it 0.
pub fn e_value(stat: &Statistic, bound: &Bound, obs: &[f64]) -> Result<f64, Error> {
    let (mut p, _, _) = test(stat, bound)?;
    if obs.iter().any(|&x| p.observe(x).is_err()) {
        return Ok(0.0);
    }
    Ok(p.value())
}

/// The point estimate of the same test: the mean of `obs` lies on the side its e-process bets
/// on, at the value it tests, or past it. An exploratory claim routes on this.
pub fn holds(stat: &Statistic, bound: &Bound, obs: &[f64]) -> Result<bool, Error> {
    let (_, point, tail) = test(stat, bound)?;
    Ok(on_side(mean(obs.iter().copied()), point, tail))
}

/// A seeded order of `0..n`: `Rng::sample`'s forward Fisher-Yates shuffle of all n.
pub fn order(n: usize, rng: &mut Rng) -> Vec<usize> {
    let n = n as u64;
    rng.sample(n, n).into_iter().map(|i| i as usize).collect()
}
