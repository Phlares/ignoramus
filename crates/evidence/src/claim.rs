//! The claim model. A claim is one or two one-sided bounds on a contrast of paired values; it
//! holds when every bound holds (intersection-union: Berger, Technometrics 24(4), 1982,
//! doi:10.1080/00401706.1982.10487790). Each cluster gives the test one observation of a
//! statistic.
use crate::Tail;

/// One one-sided bound: `Lower` claims the value lies below `margin`, `Upper` above it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bound {
    pub tail: Tail,
    pub margin: f64,
}

/// What each cluster gives the test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Statistic {
    /// The mean of the cluster's units, declared to lie in `[lo, hi]`.
    Mean { lo: f64, hi: f64 },
    /// The share of the cluster's units on the claimed side of the margin; for the quantile
    /// `q` (0.5 is the median).
    Quantile(f64),
}
