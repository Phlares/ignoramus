//! Alpha split over a study tree's declared hypotheses, with return of the share of
//! hypotheses that never ran (Choodari-Oskooei et al., arXiv:1902.05336).
use crate::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub levels: Vec<f64>,
    pub rejected: Vec<bool>,
    pub e_value: f64,
}

/// `weights` cover every hypothesis the tree declares. `None` in `e_values` means the
/// hypothesis never ran. A hypothesis that ran gets level `alpha * w / W_ran`; one
/// that did not gets 0. `e_value` is the weighted mean of the e-values that ran.
///
/// Validity needs one condition: whether a hypothesis ran must not depend on the data
/// its e-value comes from. The box runs triggers on spawn seeds and tests on disjoint
/// report seeds.
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
    let levels: Vec<f64> = weights
        .iter()
        .zip(e_values)
        .map(|(w, e)| {
            if e.is_some() {
                alpha * w / ran_weight
            } else {
                0.0
            }
        })
        .collect();
    let rejected = levels
        .iter()
        .zip(e_values)
        .map(|(&l, e)| l > 0.0 && e.is_some_and(|e| e >= 1.0 / l))
        .collect();
    let e_value = weights
        .iter()
        .zip(e_values)
        .filter_map(|(w, e)| e.map(|e| w / ran_weight * e))
        .sum();
    Ok(Verdict {
        levels,
        rejected,
        e_value,
    })
}
