//! Alpha split over a study tree's declared hypotheses, with return of the share of
//! hypotheses that never ran. The validity argument is this crate's own: the union bound
//! and Markov's inequality, given that the set of hypotheses that ran does not depend on
//! report data. Choodari-Oskooei et al. (arXiv:1902.05336) give context on adding arms.
use crate::Error;

/// Outcome of one tree decision.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    /// Level of each hypothesis, in the order of the weights.
    pub levels: Vec<f64>,
    /// Whether each hypothesis is rejected, in the order of the weights.
    pub rejected: Vec<bool>,
    /// The merged tree e-value: the largest share-weighted e-value.
    pub e_value: f64,
}

/// `weights` cover every hypothesis the tree declares. `None` in `e_values` means the
/// hypothesis never ran. A hypothesis that ran gets level `alpha * (w / W_ran)`; one
/// that did not gets 0. `e_value` is the largest `(w / W_ran) * e` over the hypotheses that
/// ran. It is at most their weighted mean, so it is an e-value when every hypothesis that
/// ran is null, and it reaches `1 / alpha` exactly when some hypothesis is rejected.
///
/// Validity needs one condition: the set of hypotheses that ran must not depend on the
/// data any of their e-values come from. For example, decide which hypotheses run from
/// one set of seeds and compute every e-value from a disjoint set.
pub fn decide(alpha: f64, weights: &[f64], e_values: &[Option<f64>]) -> Result<Verdict, Error> {
    if !(0.0..=1.0).contains(&alpha) {
        return Err(Error::Parameter("alpha"));
    }
    let total: f64 = weights.iter().sum();
    if weights.is_empty()
        || weights.len() != e_values.len()
        || weights.iter().any(|w| !(w.is_finite() && *w > 0.0))
        || !total.is_finite()
    {
        return Err(Error::Parameter("weights"));
    }
    if e_values.iter().flatten().any(|e| e.is_nan() || *e < 0.0) {
        return Err(Error::Parameter("e-value"));
    }
    let ran_weight: f64 = weights
        .iter()
        .zip(e_values)
        .filter(|(_, e)| e.is_some())
        .map(|(w, _)| w)
        .sum();
    if ran_weight == 0.0 {
        return Ok(Verdict {
            levels: vec![0.0; weights.len()],
            rejected: vec![false; weights.len()],
            e_value: 1.0,
        });
    }
    // Each share w / W_ran is in [0, 1], so neither product below overflows; a share
    // that underflows to 0 only lowers a level.
    let share = |w: f64| w / ran_weight;
    let levels: Vec<f64> = weights
        .iter()
        .zip(e_values)
        .map(|(&w, e)| if e.is_some() { alpha * share(w) } else { 0.0 })
        .collect();
    let rejected: Vec<bool> = levels
        .iter()
        .zip(e_values)
        .map(|(&l, e)| l > 0.0 && e.is_some_and(|e| e >= 1.0 / l))
        .collect();
    // `max` skips the NaN of a share that underflows to 0 times an infinite e-value.
    let e_value = (weights.iter().zip(e_values))
        .filter_map(|(&w, e)| e.map(|e| share(w) * e))
        .fold(0.0, f64::max);
    // In exact arithmetic `share * e` reaches 1/alpha exactly when `e` reaches its level's
    // 1/level; the clamp fixes the last bit in either direction.
    let bar = 1.0 / alpha;
    let e_value = if rejected.iter().any(|&r| r) {
        e_value.max(bar)
    } else {
        e_value.min(bar.next_down())
    };
    Ok(Verdict {
        levels,
        rejected,
        e_value,
    })
}
