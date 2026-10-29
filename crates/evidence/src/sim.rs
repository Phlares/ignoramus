//! Null-tree simulation suite: family-wise error, false discovery rate and power of the
//! spawn policy, built from this crate's own e-process, tree split and e-LOND stream.
//! Per-scenario error rates come from simulation over a grid of nuisance values, with
//! Monte Carlo error reported (FDA, "Adaptive Designs for Clinical Trials of Drugs and
//! Biologics", 2019).
use crate::elond::{Gamma, Stream};
use crate::eprocess::{EProcess, Score};
use crate::rng::Rng;
use crate::tree::decide;
use crate::{Error, Tail};

/// 97.5th percentile of the standard normal.
const Z: f64 = 1.959963984540054;
const SCORE: Score = Score::Clip { scale: 1.0 };

/// One simulated setting. Declared nodes are numbered breadth-first: the root is 0 and
/// node k's children are `children * k + 1 ..= children * k + children`.
#[derive(Debug, Clone, PartialEq)]
pub struct Scenario {
    pub alpha: f64,
    pub gamma: Gamma,
    /// Trees per replicate, one stream slot each.
    pub trees: usize,
    /// Declared children of each node above `depth`.
    pub children: usize,
    /// Levels of declared nodes below the root, 0 to 2.
    pub depth: u8,
    /// Units in each data role (spawn and report).
    pub units: usize,
    /// Correlation between nodes through one shared factor per unit.
    pub rho: f64,
    /// Trigger threshold on a node's spawn e-process.
    pub theta: f64,
    /// Chance that a tree carries the effect.
    pub non_null: f64,
    /// Mean of a node that carries the effect.
    pub effect: f64,
    /// Only the root carries the effect.
    pub root_only: bool,
    /// Planted defect: null children each firing node runs outside the split, each
    /// tested alone at the slot's full level.
    pub undeclared: usize,
}

/// Mean of `n` replicate results in [0, 1] and the upper end of its 95% Wilson score
/// interval (Wilson 1927; Brown, Cai and DasGupta 2001).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    pub mean: f64,
    pub upper: f64,
    pub n: usize,
}

impl Estimate {
    /// `n = 0` gives mean 0 and upper end 1.
    pub fn of(sum: f64, n: usize) -> Estimate {
        if n == 0 {
            return Estimate {
                mean: 0.0,
                upper: 1.0,
                n,
            };
        }
        let n_f = n as f64;
        let mean = sum / n_f;
        let p = mean.clamp(0.0, 1.0);
        let z2 = Z * Z / n_f;
        let half = Z * (p * (1.0 - p) / n_f + z2 / (4.0 * n_f)).sqrt();
        Estimate {
            mean,
            upper: (p + z2 / 2.0 + half) / (1.0 + z2),
            n,
        }
    }

    /// The bound holds when the upper end is at most `target`.
    pub fn holds(&self, target: f64) -> bool {
        self.upper <= target
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Report {
    /// Replicates that reject any null hypothesis.
    pub fwer: Estimate,
    /// Share of discovered trees without an effect.
    pub fdr: Estimate,
    /// Share of trees with an effect that are discovered, over replicates that have one.
    pub power: Estimate,
}

/// Runs `reps` replicates, each one stream of `s.trees` trees in order.
///
/// A hypothesis is null when its node's mean is 0; a tree has an effect when its root's
/// mean is not 0. An undeclared rejection counts as a discovery of its tree but not as
/// one of the stream's.
pub fn simulate(s: &Scenario, reps: usize, rng: &mut Rng) -> Result<Report, Error> {
    let new_stream = || {
        Stream::new(s.alpha, s.gamma).map_err(|e| match e {
            Error::Parameter("m") => Error::Parameter("gamma"),
            e => e,
        })
    };
    new_stream()?;
    validate(s, reps)?;
    let parents = parents(s)?;
    let weights = vec![1.0; s.children * parents + 1];
    let (mut fwer, mut fdr, mut power, mut powered) = (0.0, 0.0, 0.0, 0);
    for _ in 0..reps {
        let mut stream = new_stream()?;
        let mut false_rejection = false;
        let (mut found, mut false_found, mut true_found, mut effects) = (0, 0, 0, 0);
        for _ in 0..s.trees {
            let slot = stream.open();
            let level = slot.level();
            let carries = rng.uniform() <= s.non_null;
            let (e_values, stray) = run_tree(s, parents, carries, level, rng)?;
            let verdict = decide(level, &weights, &e_values)?;
            let null_rejected = (verdict.rejected.iter().enumerate())
                .any(|(k, &r)| r && node_mean(s, carries, k) == 0.0);
            false_rejection |= null_rejected || stray > 0;
            let discovered = stream.close(slot, verdict.e_value)? || stray > 0;
            let has_effect = node_mean(s, carries, 0) != 0.0;
            found += usize::from(discovered);
            false_found += usize::from(discovered && !has_effect);
            true_found += usize::from(discovered && has_effect);
            effects += usize::from(has_effect);
        }
        fwer += f64::from(u8::from(false_rejection));
        fdr += false_found as f64 / found.max(1) as f64;
        if effects > 0 {
            power += true_found as f64 / effects as f64;
            powered += 1;
        }
    }
    Ok(Report {
        fwer: Estimate::of(fwer, reps),
        fdr: Estimate::of(fdr, reps),
        power: Estimate::of(power, powered),
    })
}

/// Validates the fields the stream does not.
fn validate(s: &Scenario, reps: usize) -> Result<(), Error> {
    let checks = [
        (s.trees >= 1, "trees"),
        (s.depth <= 2, "depth"),
        (s.units >= 1, "units"),
        ((0.0..1.0).contains(&s.rho), "rho"),
        (s.theta > 1.0, "theta"),
        ((0.0..=1.0).contains(&s.non_null), "non_null"),
        (s.effect.is_finite(), "effect"),
        (reps >= 1, "reps"),
    ];
    match checks.iter().find(|(ok, _)| !ok) {
        Some(&(_, name)) => Err(Error::Parameter(name)),
        None => Ok(()),
    }
}

/// Number of declared nodes above `depth`.
fn parents(s: &Scenario) -> Result<usize, Error> {
    let grow = |p: usize| s.children.checked_mul(p)?.checked_add(1);
    (0..s.depth)
        .try_fold(0, |p, _| grow(p))
        .filter(|&p| grow(p).is_some())
        .ok_or(Error::Parameter("children"))
}

fn node_mean(s: &Scenario, carries: bool, k: usize) -> f64 {
    if carries && (k == 0 || !s.root_only) {
        s.effect
    } else {
        0.0
    }
}

/// Draws one tree's data and runs its nodes. Returns each declared node's report
/// e-value (`None` when it did not run) and the number of undeclared children
/// rejected at `level`.
fn run_tree(
    s: &Scenario,
    parents: usize,
    carries: bool,
    level: f64,
    rng: &mut Rng,
) -> Result<(Vec<Option<f64>>, usize), Error> {
    let spawn = Role::draw(s, rng);
    let report = Role::draw(s, rng);
    let nodes = s.children * parents + 1;
    let mut ran = vec![false; nodes];
    ran[0] = true;
    let mut e_values = vec![None; nodes];
    let mut stray = 0;
    for k in 0..nodes {
        if !ran[k] {
            continue;
        }
        let mean = node_mean(s, carries, k);
        e_values[k] = Some(report.e_value(mean, rng)?);
        if k < parents && spawn.fires(s.theta, mean, rng)? {
            ran[s.children * k + 1..=s.children * k + s.children].fill(true);
            for _ in 0..s.undeclared {
                let e = report.e_value(0.0, rng)?;
                stray += usize::from(decide(level, &[1.0], &[Some(e)])?.rejected[0]);
            }
        }
    }
    Ok((e_values, stray))
}

/// One data role of one tree: `sqrt(rho) Z_i` per unit, shared by every node, and the
/// scale of each node's own noise.
struct Role {
    shared: Vec<f64>,
    own: f64,
}

impl Role {
    fn draw(s: &Scenario, rng: &mut Rng) -> Role {
        let a = s.rho.sqrt();
        Role {
            shared: (0..s.units).map(|_| a * rng.normal()).collect(),
            own: (1.0 - s.rho).sqrt(),
        }
    }

    fn feed(&self, mut p: EProcess, mean: f64, rng: &mut Rng) -> Result<EProcess, Error> {
        for z in &self.shared {
            p.observe(mean + z + self.own * rng.normal())?;
        }
        Ok(p)
    }

    fn fires(&self, theta: f64, mean: f64, rng: &mut Rng) -> Result<bool, Error> {
        let p = EProcess::new(SCORE, Tail::Upper, theta)?;
        Ok(self.feed(p, mean, rng)?.stopped())
    }

    fn e_value(&self, mean: f64, rng: &mut Rng) -> Result<f64, Error> {
        let p = EProcess::new(SCORE, Tail::Both, f64::INFINITY)?;
        Ok(self.feed(p, mean, rng)?.value())
    }
}
