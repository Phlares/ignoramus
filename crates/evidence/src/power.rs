//! Power of the claim test, by simulation. Each replicate draws every stratum's clusters from
//! a normal model of clustered units and runs the test of [`crate::claim`] on them. The claim
//! passes a replicate only when every bound in every stratum reaches 1 / level
//! (intersection-union, each test at the full level), so the power is that of the joint event.
//! A call draws at most [`WORK`] values, so a check that sizes a study cannot run for hours.
//! [`staged`] simulates a design with a blinded re-size from a reserve.
use crate::claim::{Bound, Statistic, e_values, observation, order};
use crate::rng::Rng;
use crate::{Error, rejects};

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

/// The most values one call of [`simulate`], [`clusters_needed`] or [`staged`] may draw.
pub const WORK: u64 = 2_000_000_000;

/// The most clusters per stratum the search for the count that would pass tries.
pub const CAP: u32 = 16_384;

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
        e_values(&plan.stat, b, &[])?;
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
    charge(&mut 0, work(plan, reps))?;
    Ok(share(run(plan, reps, rng, |_, _| false)?, reps))
}

/// Adds `values` to `spent`; `Error::Parameter("work")` when the sum passes [`WORK`].
fn charge(spent: &mut u64, values: u64) -> Result<(), Error> {
    *spent = spent.saturating_add(values);
    if *spent > WORK {
        return Err(Error::Parameter("work"));
    }
    Ok(())
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

/// One stratum of one replicate: the clusters in a seeded order, then each bound's test.
fn stratum_passes(plan: &Plan, s: &Stratum, rng: &mut Rng) -> Result<bool, Error> {
    let mut obs = vec![Vec::with_capacity(s.sizes.len()); plan.bounds.len()];
    let seeded = order(s.sizes.len(), rng);
    observe(plan, s, seeded, rng, &mut obs, &unit_model)?;
    Ok(holds_all(plan, &obs)?.last)
}

/// Fills `out` with the values of one cluster of `m` units of stratum `s`.
type Draw<'a> = &'a dyn Fn(&Statistic, &Stratum, u32, &mut Rng, &mut Vec<f64>);

/// Draws the clusters of `s` at `indices` by `draw`, in that order, and adds each one's
/// observation for each bound to that bound's list in `obs`.
fn observe(
    plan: &Plan,
    s: &Stratum,
    indices: impl IntoIterator<Item = usize>,
    rng: &mut Rng,
    obs: &mut [Vec<f64>],
    draw: Draw,
) -> Result<(), Error> {
    let mut values = Vec::new();
    for i in indices {
        draw(&plan.stat, s, s.sizes[i], rng, &mut values);
        for (b, o) in plan.bounds.iter().zip(&mut *obs) {
            o.push(observation(&plan.stat, b, &values)?);
        }
    }
    Ok(())
}

/// What the tests reached: `last`, every bound's e-value reaches 1 / level after the last
/// observation; `ever`, every bound's reaches it after some observation.
#[derive(Debug, Clone, Copy)]
struct Reached {
    last: bool,
    ever: bool,
}

/// What each bound's test over its observations reached.
fn holds_all(plan: &Plan, obs: &[Vec<f64>]) -> Result<Reached, Error> {
    let mut last = true;
    for (b, o) in plan.bounds.iter().zip(obs) {
        let (e, peak) = e_values(&plan.stat, b, o)?;
        // The peak is at least the last value, so a bound that never reaches 1 / level ends it.
        if !rejects(peak, plan.level) {
            return Ok(Reached {
                last: false,
                ever: false,
            });
        }
        last &= rejects(e, plan.level);
    }
    Ok(Reached { last, ever: true })
}

/// The values one cluster of `m` units gives `observation`. A mean's observation is the mean
/// of the units, which under the unit model is normal with sd `mean_sd`: one draw stands for
/// the m units, and `observation` clips it to the range as it clips each unit of a real
/// cluster. The two agree while the units stay inside the range. A quantile's observation
/// needs every unit.
fn unit_model(stat: &Statistic, s: &Stratum, m: u32, rng: &mut Rng, out: &mut Vec<f64>) {
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
    needed(plan, target, reps, seed, cap, &mut 0)
}

/// [`clusters_needed`], its trials charged to `spent`.
fn needed(
    plan: &Plan,
    target: f64,
    reps: u32,
    seed: [u8; 32],
    cap: u32,
    spent: &mut u64,
) -> Result<Option<u32>, Error> {
    validate(plan, reps)?;
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
        charge(spent, work(&trial, reps))?;
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

/// The blinded sd of one stratum's cluster observations: the root mean square, sqrt(mean(x^2)),
/// taken about 0 so it never reads the effect. It is at least the centred sd, by the squared
/// mean. `None` when it is not finite, as for no observation or a non-finite one.
pub fn blinded_sd(obs: &[f64]) -> Option<f64> {
    let s = (obs.iter().map(|x| x * x).sum::<f64>() / obs.len() as f64).sqrt();
    s.is_finite().then_some(s)
}

/// The re-size count: [`clusters_needed`] for `plan`'s claim on one stratum of `stratum_sizes`
/// whose cluster observations have sd `s` about `unit_mean`. Its icc of 1 makes `s` the sd of
/// every cluster's observation, whatever its units.
#[allow(clippy::too_many_arguments)]
pub fn resize_count(
    plan: &Plan,
    stratum_sizes: &[u32],
    s: f64,
    unit_mean: f64,
    target: f64,
    reps: u32,
    seed: [u8; 32],
    cap: u32,
) -> Result<Option<u32>, Error> {
    let one = one_stratum(plan, stratum_sizes, s, unit_mean);
    clusters_needed(&one, target, reps, seed, cap)
}

fn one_stratum(plan: &Plan, sizes: &[u32], s: f64, unit_mean: f64) -> Plan {
    Plan {
        strata: vec![Stratum {
            sizes: sizes.to_vec(),
            sd: s,
            icc: 1.0,
            unit_mean,
        }],
        ..plan.clone()
    }
}

/// One stratum of a design with a blinded re-size. The data are drawn about `unit_mean`; the
/// re-size count assumes `size_at`.
#[derive(Debug, Clone, PartialEq)]
pub struct StagedStratum {
    /// The units of each first-batch cluster.
    pub first: Vec<u32>,
    /// The units of each reserve cluster, in the order the re-size appends them.
    pub reserve: Vec<u32>,
    pub sd: f64,
    pub icc: f64,
    pub unit_mean: f64,
    pub size_at: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Staged {
    pub stat: Statistic,
    pub bounds: Vec<Bound>,
    pub strata: Vec<StagedStratum>,
    /// The level of each bound's test.
    pub level: f64,
    /// The power the re-size asks of each stratum.
    pub target_each: f64,
}

/// The share of `reps` replicates of the whole design that pass; see [`staged_detail`].
pub fn staged(plan: &Staged, reps: u32, seed: [u8; 32]) -> Result<f64, Error> {
    Ok(staged_detail(plan, reps, seed)?.pass)
}

/// Shares of the replicates of a staged design.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shares {
    /// Every bound in every stratum reaches 1 / level after the last observation.
    pub pass: f64,
    /// The re-size stops.
    pub stop: f64,
    /// Every bound in every stratum reaches 1 / level after some observation. Under the null
    /// Ville's inequality bounds this share by the level too.
    pub crossed: f64,
}

/// The shares of `reps` replicates of the whole design that pass, stop and cross. A replicate
/// draws each stratum's first batch in a seeded order and takes the blinded sd `s` of its
/// observations for the first bound (a mean gives every bound the same one). The stratum's
/// table gives the count that `s` needs; the replicate stops, and fails, when a stratum has no
/// count. Otherwise each stratum appends reserve clusters in reserve order until it holds its
/// count, and every bound in every stratum must reach 1 / level over the first batch, then the
/// appended clusters.
///
/// A stratum's table: 8 values of `s` geometric from s0 / 4 to 4 s0, with
/// s0 = sqrt(mean_sd(sd, icc, first)^2 + size_at^2), each with its [`resize_count`] at
/// `size_at`, `target_each` and `reps`, capped at first + reserve. A replicate's `s` takes the
/// count of the first value at or above it; above the top value it has none. The table's
/// searches and the replicates run on `seed`, and draw at most [`WORK`] values in all.
pub fn staged_detail(plan: &Staged, reps: u32, seed: [u8; 32]) -> Result<Shares, Error> {
    staged_run(plan, reps, seed, &unit_model)
}

/// [`staged_detail`] with each replicate's clusters drawn by `draw`, one value per cluster, in
/// place of the unit model; the re-size table still assumes the unit model. For null
/// simulations of shapes the unit model cannot give.
pub fn staged_detail_by(
    plan: &Staged,
    reps: u32,
    seed: [u8; 32],
    draw: impl Fn(&mut Rng) -> f64,
) -> Result<Shares, Error> {
    staged_run(plan, reps, seed, &|_, _, _, rng, out| {
        out.clear();
        out.push(draw(rng));
    })
}

fn staged_run(plan: &Staged, reps: u32, seed: [u8; 32], draw: Draw) -> Result<Shares, Error> {
    let whole = Plan {
        stat: plan.stat,
        bounds: plan.bounds.clone(),
        strata: (plan.strata.iter())
            .map(|s| Stratum {
                sizes: [s.first.as_slice(), &s.reserve].concat(),
                sd: s.sd,
                icc: s.icc,
                unit_mean: s.unit_mean,
            })
            .collect(),
        level: plan.level,
    };
    validate(&whole, reps)?;
    let checks = [
        (plan.strata.iter().all(|s| !s.first.is_empty()), "sizes"),
        (plan.strata.iter().all(|s| s.size_at.is_finite()), "size_at"),
        ((0.0..=1.0).contains(&plan.target_each), "target_each"),
    ];
    if let Some(&(_, name)) = checks.iter().find(|(ok, _)| !ok) {
        return Err(Error::Parameter(name));
    }
    let mut spent = 0;
    charge(&mut spent, work(&whole, reps))?;
    // A stratum's re-size table: each grid value of `s` with its count.
    let mut table = |st: &StagedStratum, s: &Stratum| {
        let s0 = mean_sd(st.sd, st.icc, &st.first).hypot(st.size_at);
        // `WORK` bounds the clusters below 2^31: each draws a value per replicate.
        let cap = s.sizes.len() as u32;
        (0..8)
            .map(|i| {
                let at = s0 / 4.0 * 16f64.powf(f64::from(i) / 7.0);
                let one = one_stratum(&whole, &s.sizes, at, st.size_at);
                Ok((
                    at,
                    needed(&one, plan.target_each, reps, seed, cap, &mut spent)?,
                ))
            })
            .collect::<Result<Vec<_>, Error>>()
    };
    let tables = (plan.strata.iter().zip(&whole.strata))
        .map(|(st, s)| table(st, s))
        .collect::<Result<Vec<_>, _>>()?;
    let mut rng = Rng::from_seed(seed);
    let (mut passed, mut stopped, mut crossed) = (0, 0, 0);
    for _ in 0..reps {
        match staged_replicate(&whole, plan, &tables, &mut rng, draw)? {
            Some(r) => {
                passed += u32::from(r.last);
                crossed += u32::from(r.ever);
            }
            None => stopped += 1,
        }
    }
    Ok(Shares {
        pass: share(passed, reps),
        stop: share(stopped, reps),
        crossed: share(crossed, reps),
    })
}

/// What the replicate's tests reached, or `None` when the re-size stops.
fn staged_replicate(
    whole: &Plan,
    plan: &Staged,
    tables: &[Vec<(f64, Option<u32>)>],
    rng: &mut Rng,
    draw: Draw,
) -> Result<Option<Reached>, Error> {
    let mut sized = Vec::with_capacity(tables.len());
    for ((st, s), table) in plan.strata.iter().zip(&whole.strata).zip(tables) {
        let mut obs = vec![Vec::with_capacity(s.sizes.len()); plan.bounds.len()];
        let seeded = order(st.first.len(), rng);
        observe(whole, s, seeded, rng, &mut obs, draw)?;
        let count = blinded_sd(&obs[0])
            .and_then(|sd| table.iter().find(|&&(at, _)| at >= sd))
            .and_then(|&(_, n)| n);
        let Some(n) = count else {
            return Ok(None);
        };
        sized.push((s, obs, (n as usize).max(st.first.len())));
    }
    let mut last = true;
    for ((s, mut obs, n), st) in sized.into_iter().zip(&plan.strata) {
        observe(whole, s, st.first.len()..n, rng, &mut obs, draw)?;
        let r = holds_all(whole, &obs)?;
        if !r.ever {
            return Ok(Some(r));
        }
        last &= r.last;
    }
    Ok(Some(Reached { last, ever: true }))
}
