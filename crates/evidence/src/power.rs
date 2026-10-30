//! Power of the claim test, by simulation. Each replicate draws every stratum's clusters from
//! a normal model of clustered units and runs the test of [`crate::claim`] on them. The claim
//! passes a replicate only when every bound in every stratum reaches 1 / level
//! (intersection-union, each test at the full level), so the power is that of the joint event.
//! A call draws at most [`WORK`] values, so a check that sizes a study ends in seconds.
use crate::Error;
use crate::claim::{Bound, Statistic, e_value, observation, order};
use crate::rng::Rng;

/// The clusters of one stratum and their unit model: unit x = `unit_mean` + `sd` (sqrt(icc)
/// Z_c + sqrt(1 - icc) N_u), with Z_c per cluster and N_u per unit, both standard normal.
#[derive(Debug, Clone, PartialEq)]
pub struct Stratum {
    /// The units of each cluster.
    pub sizes: Vec<u32>,
    pub sd: f64,
    pub icc: f64,
    pub unit_mean: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub stat: Statistic,
    pub bounds: Vec<Bound>,
    pub strata: Vec<Stratum>,
    /// The level of each bound's test.
    pub level: f64,
}

/// The most values one call of [`simulate`] or [`clusters_needed`] may draw.
pub const WORK: u64 = 100_000_000;

/// The values `reps` replicates of `plan` draw: for a mean one per cluster, for a quantile one
/// per cluster and one per unit.
pub fn work(plan: &Plan, reps: u32) -> u64 {
    let per_cluster = |m: u32| match plan.stat {
        Statistic::Mean { .. } => 1,
        Statistic::Quantile(_) => 1 + u64::from(m),
    };
    (plan.strata.iter().flat_map(|s| &s.sizes))
        .fold(0u64, |sum, &m| sum.saturating_add(per_cluster(m)))
        .saturating_mul(u64::from(reps))
}

/// The sd of a cluster mean under the unit model, sd sqrt(icc + (1 - icc) / m), as the root
/// of its variance averaged over clusters of `sizes`. The mean of k cluster means has that
/// variance over k, so for several sizes this is the sd at their harmonic mean.
pub fn mean_sd(sd: f64, icc: f64, sizes: &[u32]) -> f64 {
    let inverse = sizes.iter().map(|&m| 1.0 / f64::from(m)).sum::<f64>() / sizes.len() as f64;
    sd * (icc + (1.0 - icc) * inverse).sqrt()
}

fn validate(plan: &Plan, reps: u32) -> Result<(), Error> {
    if !(plan.level > 0.0 && plan.level < 1.0) {
        return Err(Error::Parameter("level"));
    }
    if plan.bounds.is_empty() {
        return Err(Error::Parameter("bounds"));
    }
    for b in &plan.bounds {
        e_value(&plan.stat, b, &[])?;
    }
    if plan.strata.is_empty() {
        return Err(Error::Parameter("strata"));
    }
    for s in &plan.strata {
        let checks = [
            (!s.sizes.is_empty() && !s.sizes.contains(&0), "sizes"),
            ((0.0..=1.0).contains(&s.icc), "icc"),
            (s.sd.is_finite() && s.sd >= 0.0, "sd"),
            (s.unit_mean.is_finite(), "unit_mean"),
        ];
        if let Some(&(_, name)) = checks.iter().find(|(ok, _)| !ok) {
            return Err(Error::Parameter(name));
        }
    }
    if reps == 0 {
        return Err(Error::Parameter("reps"));
    }
    Ok(())
}

/// The share of `reps` replicates in which every bound in every stratum reaches 1 / level.
/// `Error::Parameter("work")` when they would draw more than [`WORK`] values.
pub fn simulate(plan: &Plan, reps: u32, rng: &mut Rng) -> Result<f64, Error> {
    validate(plan, reps)?;
    if work(plan, reps) > WORK {
        return Err(Error::Parameter("work"));
    }
    Ok(share(run(plan, reps, rng, |_, _| false)?, reps))
}

fn share(passed: u32, reps: u32) -> f64 {
    f64::from(passed) / f64::from(reps)
}

/// The replicates that pass, out of the first `reps`, or of fewer once `decided(passed, done)`
/// holds after `done` replicates.
fn run(
    plan: &Plan,
    reps: u32,
    rng: &mut Rng,
    decided: impl Fn(u32, u32) -> bool,
) -> Result<u32, Error> {
    let mut passed = 0;
    for done in 1..=reps {
        passed += u32::from(replicate(plan, rng)?);
        if decided(passed, done) {
            break;
        }
    }
    Ok(passed)
}

fn replicate(plan: &Plan, rng: &mut Rng) -> Result<bool, Error> {
    for s in &plan.strata {
        if !stratum_passes(plan, s, rng)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// One stratum of one replicate: the clusters in a seeded order, each giving one observation
/// per bound, then each bound's test.
fn stratum_passes(plan: &Plan, s: &Stratum, rng: &mut Rng) -> Result<bool, Error> {
    let mut obs = vec![Vec::with_capacity(s.sizes.len()); plan.bounds.len()];
    let mut values = Vec::new();
    for i in order(s.sizes.len(), rng) {
        draw(&plan.stat, s, s.sizes[i], rng, &mut values);
        for (b, o) in plan.bounds.iter().zip(&mut obs) {
            o.push(observation(&plan.stat, b, &values)?);
        }
    }
    for (b, o) in plan.bounds.iter().zip(&obs) {
        if e_value(&plan.stat, b, o)? < 1.0 / plan.level {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The values one cluster of `m` units gives `observation`. A mean's observation is the mean
/// of the units, which under the unit model is normal with sd `mean_sd`: one draw stands for
/// the m units, and `observation` clips it to the range as it clips each unit of a real
/// cluster. The two agree while the units stay inside the range. A quantile's observation
/// needs every unit.
fn draw(stat: &Statistic, s: &Stratum, m: u32, rng: &mut Rng, out: &mut Vec<f64>) {
    out.clear();
    match stat {
        Statistic::Mean { .. } => {
            out.push(s.unit_mean + mean_sd(s.sd, s.icc, &[m]) * rng.normal());
        }
        Statistic::Quantile(_) => {
            let shared = s.unit_mean + s.sd * s.icc.sqrt() * rng.normal();
            let own = s.sd * (1.0 - s.icc).sqrt();
            out.extend((0..m).map(|_| shared + own * rng.normal()));
        }
    }
}

/// The smallest common count n of clusters per stratum whose simulated power reaches `target`;
/// `None` when `cap` clusters do not. A trial gives each stratum its declared sizes repeated in
/// order up to n, so the trial at the present count is the present plan. Every trial runs on
/// the same seed. The search doubles from 1 until a count passes, then bisects between the
/// last count that failed and the first that passed (no cluster always fails).
/// `Error::Parameter("work")` when the trials would draw more than [`WORK`] values in all.
pub fn clusters_needed(
    plan: &Plan,
    target: f64,
    reps: u32,
    seed: [u8; 32],
    cap: u32,
) -> Result<Option<u32>, Error> {
    validate(plan, reps)?;
    let mut spent = 0u64;
    let mut passes = |n: u32| -> Result<bool, Error> {
        let strata = (plan.strata.iter())
            .map(|s| Stratum {
                sizes: s.sizes.iter().copied().cycle().take(n as usize).collect(),
                ..s.clone()
            })
            .collect();
        let trial = Plan {
            strata,
            ..plan.clone()
        };
        spent = spent.saturating_add(work(&trial, reps));
        if spent > WORK {
            return Err(Error::Parameter("work"));
        }
        // `simulate(&trial, ..) >= target`, stopped once the replicates left cannot change it.
        let reached = |passed: u32| share(passed, reps) >= target;
        let passed = run(&trial, reps, &mut Rng::from_seed(seed), |passed, done| {
            reached(passed) || !reached(passed + (reps - done))
        })?;
        Ok(reached(passed))
    };
    if cap == 0 {
        return Ok(None);
    }
    let (mut fails, mut hi) = (0, 1);
    while !passes(hi)? {
        if hi == cap {
            return Ok(None);
        }
        (fails, hi) = (hi, hi.saturating_mul(2).min(cap));
    }
    while hi - fails > 1 {
        let mid = fails + (hi - fails) / 2;
        if passes(mid)? {
            hi = mid;
        } else {
            fails = mid;
        }
    }
    Ok(Some(hi))
}
