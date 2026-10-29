//! Local and global (look-elsewhere) p-values for a scan over many conditions.
//! Single-step maxT (Westfall and Young 1993) with the +1 correction of
//! Phipson and Smyth (2010).

use crate::rng::Rng;
use crate::{Error, Tail};

/// P-values per condition: `local` for one condition, `global` over the whole scan.
#[derive(Debug, Clone, PartialEq)]
pub struct Scan {
    pub local: Vec<f64>,
    pub global: Vec<f64>,
}

/// Local and global p-values from null statistics, oriented so larger is
/// more extreme. Ties count as extreme. An empty `null` gives p-values of 1.
pub fn from_null(observed: &[f64], null: &[Vec<f64>]) -> Result<Scan, Error> {
    if null.iter().any(|row| row.len() != observed.len()) {
        return Err(Error::Parameter("null"));
    }
    if observed
        .iter()
        .chain(null.iter().flatten())
        .any(|v| !v.is_finite())
    {
        return Err(Error::NotFinite);
    }
    let mut local = vec![1usize; observed.len()];
    let mut global = vec![1usize; observed.len()];
    for row in null {
        let max = row.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        for (k, &obs) in observed.iter().enumerate() {
            local[k] += usize::from(row[k] >= obs);
            global[k] += usize::from(max >= obs);
        }
    }
    let scale = (null.len() + 1) as f64;
    Ok(Scan {
        local: local.iter().map(|&c| c as f64 / scale).collect(),
        global: global.iter().map(|&c| c as f64 / scale).collect(),
    })
}

/// Sign-flip null for paired differences. `diffs[k]` holds condition k's
/// differences over the same units in the same order.
///
/// Statistic: `sum(s_i d_i) / sqrt(sum(d_i^2))`, 0 when the sum of squares is
/// 0, oriented by `tail`. The observed statistic uses all signs +1. Each draw
/// takes one sign per unit from `rng.next_u64()`, 64 units per word, lowest
/// bit first; a set bit is +1. One draw's signs apply to every condition.
pub fn sign_flip(
    diffs: &[Vec<f64>],
    tail: Tail,
    draws: usize,
    rng: &mut Rng,
) -> Result<Scan, Error> {
    let n = diffs.first().map_or(0, Vec::len);
    if n == 0 || diffs.iter().any(|c| c.len() != n) {
        return Err(Error::Parameter("diffs"));
    }
    let norms: Vec<f64> = diffs
        .iter()
        .map(|c| c.iter().map(|d| d * d).sum::<f64>().sqrt())
        .collect();
    if norms.iter().any(|v| !v.is_finite()) {
        return Err(Error::NotFinite);
    }
    let stat = |c: &[f64], norm: f64, plus: &dyn Fn(usize) -> bool| {
        let z = if norm == 0.0 {
            0.0
        } else {
            let sum: f64 = c
                .iter()
                .enumerate()
                .map(|(i, &d)| if plus(i) { d } else { -d })
                .sum();
            sum / norm
        };
        match tail {
            Tail::Upper => z,
            Tail::Lower => -z,
            Tail::Both => z.abs(),
        }
    };
    let observed: Vec<f64> = diffs
        .iter()
        .zip(&norms)
        .map(|(c, &norm)| stat(c, norm, &|_| true))
        .collect();
    let mut words = vec![0u64; n.div_ceil(64)];
    let null: Vec<Vec<f64>> = (0..draws)
        .map(|_| {
            words.iter_mut().for_each(|w| *w = rng.next_u64());
            let plus = |i: usize| (words[i / 64] >> (i % 64)) & 1 == 1;
            diffs
                .iter()
                .zip(&norms)
                .map(|(c, &norm)| stat(c, norm, &plus))
                .collect()
        })
        .collect();
    from_null(&observed, &null)
}
