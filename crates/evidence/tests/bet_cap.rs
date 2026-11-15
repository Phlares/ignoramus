//! Simulations for the choice of the bet cap (decision 0022, "Open"). The two long ones are
//! ignored by default:
//! `cargo test -p evidence --release --test bet_cap -- --ignored --nocapture`.
//!
//! Score units: a "mean below 0" claim on [-1, 1] at level 0.005, so an observation is its own
//! score. A law draws each cluster observation at the ceiling, +1, with chance `ceiling`, and
//! otherwise normal with sd `spread` about a mean, clipped to the range. A power scenario puts
//! that mean at -`gap`. The null of a law puts it where the mean of all observations is 0, so
//! a null does not depend on the gap.
//!
//! Candidates: the caps 1/2, 3/4 and 0.9, and the three together (`EProcess::with_caps`: the
//! mean of their e-values). Every candidate reads the same draws.
//!
//! Power: per candidate and target, the smallest count of clusters at which the share of
//! replicates with an e-value of at least 1 / level reaches the target. One path per replicate
//! serves every count, since a prefix of independent draws has the law of a shorter study. The
//! count is the first one at which a simulated share touches the target, so it sits a little
//! below the true count; the lowest and highest batch beside it show the noise.
//! Null: the share of replicates whose e-value reaches 1 / level at any count up to the
//! horizon. Ville's inequality bounds that share by the level, and it bounds every fixed count.
//! A candidate keeps the level when the upper end of the share's 95% interval is at most the
//! level. With ceiling units the share lies near the bound, so those nulls take 1,500,000
//! replicates; the others take 60,000.
//!
//! Seeds: batch b runs on `seed("power/<scenario>/<b>")` or `seed("null/<law>/<b>")`.
mod support;

use evidence::Tail;
use evidence::claim::{Bound, Statistic, e_value};
use evidence::eprocess::{EProcess, MAX_BET, Score};
use evidence::rng::Rng;
use evidence::sim::Estimate;
use std::ops::Range;
use support::{par_map, seed};

const LEVEL: f64 = 0.005;
const CAPS: [f64; 3] = [0.5, MAX_BET, 0.9];
/// Each cap alone, then the three together.
const N: usize = CAPS.len() + 1;
const NAMES: [&str; N] = ["1/2", "3/4", "0.9", "mixture"];
/// One stratum's share of a joint power of 0.8 over 1, 2 and 4 strata.
const TARGETS: [f64; 3] = [0.8, 0.894427, 0.945742];
const HORIZON: usize = evidence::power::CAP as usize;
const BATCH: u32 = 1_000;
const SCORE: Score = Score::Bounded {
    lo: -1.0,
    hi: 1.0,
    mean: 0.0,
};

#[derive(Clone, Copy)]
struct Law {
    spread: f64,
    ceiling: f64,
}

impl Law {
    fn name(&self) -> String {
        format!("spread {}, ceiling {}", self.spread, self.ceiling)
    }

    /// The mean of the normal part that puts the mean of all observations at 0.
    fn null_mean(&self) -> f64 {
        -self.ceiling / (1.0 - self.ceiling)
    }

    fn draw(&self, mean: f64, rng: &mut Rng) -> f64 {
        if rng.uniform() < self.ceiling {
            1.0
        } else {
            (mean + self.spread * rng.normal()).clamp(-1.0, 1.0)
        }
    }
}

#[derive(Clone, Copy)]
struct Scenario {
    gap: f64,
    law: Law,
}

impl Scenario {
    fn name(&self) -> String {
        format!("gap {}, {}", self.gap, self.law.name())
    }
}

/// The candidates over one path.
struct Candidates([EProcess; N]);

impl Candidates {
    fn new() -> Candidates {
        let test =
            |caps: &[f64]| EProcess::with_caps(SCORE, Tail::Lower, f64::INFINITY, caps).unwrap();
        Candidates([
            test(&CAPS[..1]),
            test(&CAPS[1..2]),
            test(&CAPS[2..]),
            test(&CAPS),
        ])
    }

    fn observe(&mut self, x: f64) {
        for p in &mut self.0 {
            p.observe(x).unwrap();
        }
    }

    fn values(&self) -> [f64; N] {
        self.0.each_ref().map(EProcess::value)
    }

    fn peaks(&self) -> [f64; N] {
        self.0.each_ref().map(EProcess::peak)
    }
}

fn reaches(e: f64) -> bool {
    evidence::rejects(e, LEVEL)
}

/// `passed[c][n]`: the replicates in which candidate c reaches 1 / level after exactly n
/// observations about -`gap`.
fn passes(s: &Scenario, reps: u32, horizon: usize, seed: [u8; 32]) -> [Vec<u32>; N] {
    let mut rng = Rng::from_seed(seed);
    let mut passed = [(); N].map(|()| vec![0; horizon + 1]);
    for _ in 0..reps {
        let mut c = Candidates::new();
        for n in 1..=horizon {
            c.observe(s.law.draw(-s.gap, &mut rng));
            for (p, v) in passed.iter_mut().zip(c.values()) {
                p[n] += u32::from(reaches(v));
            }
        }
    }
    passed
}

/// The replicates in which each candidate reaches 1 / level at any count up to `horizon`,
/// with the normal part about `mean`.
fn crossed(law: &Law, mean: f64, reps: u32, horizon: usize, seed: [u8; 32]) -> [u32; N] {
    let mut rng = Rng::from_seed(seed);
    let mut crossed = [0; N];
    for _ in 0..reps {
        let mut c = Candidates::new();
        for _ in 0..horizon {
            c.observe(law.draw(mean, &mut rng));
        }
        for (k, peak) in crossed.iter_mut().zip(c.peaks()) {
            *k += u32::from(reaches(peak));
        }
    }
    crossed
}

/// The pass counts of several batches, summed per candidate and count.
fn pool(batches: &[[Vec<u32>; N]]) -> [Vec<u32>; N] {
    std::array::from_fn(|c| {
        (0..batches[0][c].len())
            .map(|n| batches.iter().map(|b| b[c][n]).sum())
            .collect()
    })
}

/// The smallest count at which the pass share reaches `target`.
fn needed(passed: &[u32], reps: u32, target: f64) -> Option<usize> {
    (passed.iter()).position(|&k| f64::from(k) / f64::from(reps) >= target)
}

const SHARP: Law = Law {
    spread: 0.3,
    ceiling: 0.01,
};

#[test]
fn the_cap_in_force_gives_the_claim_test() {
    let stat = Statistic::Mean { lo: -1.0, hi: 1.0 };
    let bound = Bound {
        tail: Tail::Lower,
        margin: 0.0,
    };
    let mut rng = Rng::from_seed(seed("claim"));
    let obs: Vec<f64> = (0..400).map(|_| SHARP.draw(-0.03, &mut rng)).collect();
    let mut c = Candidates::new();
    obs.iter().for_each(|&x| c.observe(x));
    let last = c.values();
    assert_eq!(last[1], e_value(&stat, &bound, &obs).unwrap());
    assert!(last[0] != last[1] && last[2] != last[1], "{last:?}");
    let mean = (last[0] + last[1] + last[2]) / 3.0;
    assert!((last[3] - mean).abs() <= 1e-12 * mean, "{last:?}");
}

#[test]
fn needed_is_the_first_count_at_the_target() {
    assert_eq!(needed(&[0, 1, 3, 2, 4], 4, 0.75), Some(2));
    assert_eq!(needed(&[0, 1, 3, 2, 4], 4, 1.0), Some(4));
    assert_eq!(needed(&[0, 1], 4, 0.5), None);
}

#[test]
fn pool_sums_the_batches_per_candidate_and_count() {
    let batch = |k: u32| std::array::from_fn(|c| vec![k, k * 10 + c as u32]);
    let pooled = pool(&[batch(1), batch(2), batch(4)]);
    for (c, counts) in pooled.iter().enumerate() {
        assert_eq!(counts, &vec![7, 70 + 3 * c as u32]);
    }
}

#[test]
fn the_null_of_a_law_has_mean_zero() {
    for ceiling in [0.0, 0.005, 0.01, 0.2] {
        let law = Law {
            spread: 0.02,
            ceiling,
        };
        let mean = ceiling + (1.0 - ceiling) * law.null_mean();
        assert!(mean.abs() < 1e-15, "{ceiling}: {mean}");
    }
    let law = Law {
        spread: 0.02,
        ceiling: 0.01,
    };
    let mut rng = Rng::from_seed(seed("twin"));
    let n = 400_000;
    let (mut sum, mut top) = (0.0, 0);
    for _ in 0..n {
        let x = law.draw(law.null_mean(), &mut rng);
        sum += x;
        top += u32::from(x == 1.0);
    }
    // One draw has sd of about 0.1, so the mean has a standard error of about 0.00016.
    let mean = sum / f64::from(n);
    assert!(mean.abs() < 0.0008, "{mean}");
    let share = f64::from(top) / f64::from(n);
    assert!((share - law.ceiling).abs() < 0.001, "{share}");
}

/// A path can reach 1 / level and fall below it again at a ceiling unit, so more paths reach
/// it at some count than hold it at the last one.
#[test]
fn crossed_counts_the_paths_that_reach_the_level_at_any_count() {
    let law = Law {
        spread: 0.3,
        ceiling: 0.03,
    };
    let (reps, horizon, mean) = (300, 200, -0.12);
    let got = crossed(&law, mean, reps, horizon, seed("crossed"));
    let mut rng = Rng::from_seed(seed("crossed"));
    let (mut ever, mut last) = ([0; N], [0; N]);
    for _ in 0..reps {
        let mut c = Candidates::new();
        let mut any = [false; N];
        for _ in 0..horizon {
            c.observe(law.draw(mean, &mut rng));
            for (a, v) in any.iter_mut().zip(c.values()) {
                *a |= reaches(v);
            }
        }
        for (i, v) in c.values().into_iter().enumerate() {
            ever[i] += u32::from(any[i]);
            last[i] += u32::from(reaches(v));
        }
    }
    assert_eq!(got, ever);
    let pairs = || ever.iter().zip(&last);
    assert!(pairs().all(|(e, l)| e >= l), "{ever:?} {last:?}");
    assert!(pairs().any(|(e, l)| e > l), "{ever:?} {last:?}");
    assert!(ever.iter().all(|&e| 0 < e && e < reps), "{ever:?}");
}

/// Decision 0022 reports about 400 and 281 clusters at caps 1/2 and 3/4 for a claim with this
/// gap at power 0.894, from a simulation outside this repository.
#[test]
fn without_ceiling_units_a_higher_cap_needs_fewer_clusters() {
    let s = Scenario {
        gap: 0.0297,
        law: Law {
            spread: 0.0266,
            ceiling: 0.0,
        },
    };
    let reps = 400;
    let passed = passes(&s, reps, 600, seed("smoke"));
    let n: Vec<usize> = (passed.iter())
        .map(|p| needed(p, reps, TARGETS[1]).unwrap())
        .collect();
    assert!(n[2] < n[1] && n[1] < n[0], "{n:?}");
    assert!((250..=310).contains(&n[1]), "{n:?}");
    assert!((360..=440).contains(&n[0]), "{n:?}");
}

/// Decision 0022's ceiling shares, and the spread of the finished study's two strata over the
/// score scale.
fn laws() -> Vec<Law> {
    let mut out = Vec::new();
    for ceiling in [0.0, 0.005, 0.01] {
        for spread in [0.0172, 0.0266] {
            out.push(Law { spread, ceiling });
        }
    }
    out
}

/// Every law at the gaps of the claims on file over their score scale.
fn grid() -> Vec<Scenario> {
    let mut out = Vec::new();
    for law in laws() {
        for gap in [0.0142, 0.0220, 0.0297] {
            out.push(Scenario { gap, law });
        }
    }
    out
}

#[test]
#[ignore = "long: 1,500,000 null replicates of 16,384 clusters for each of 4 laws with ceiling units, 60,000 for each of 2 without; run in release mode"]
fn every_candidate_keeps_the_level() {
    const CHUNK: u32 = 100;
    let laws = laws();
    let batches = |law: &Law| if law.ceiling > 0.0 { 1_500 } else { 60 };
    let chunks: Vec<(usize, Range<u32>)> = (laws.iter().enumerate())
        .flat_map(|(i, law)| {
            (0..batches(law))
                .step_by(CHUNK as usize)
                .map(move |from| (i, from..(from + CHUNK).min(batches(law))))
        })
        .collect();
    let counts = par_map(&chunks, |(i, range)| {
        let law = &laws[*i];
        range.clone().fold([0; N], |mut sum, b| {
            let name = format!("null/{}/{b}", law.name());
            let k = crossed(law, law.null_mean(), BATCH, HORIZON, seed(&name));
            sum.iter_mut().zip(k).for_each(|(n, k)| *n += k);
            sum
        })
    });
    println!(
        "null: replicates that reach 1 / {LEVEL} at any count up to {HORIZON}: count, share (upper end of its 95% interval)"
    );
    println!("law | replicates | {}", NAMES.join(" | "));
    let mut holds = true;
    for (i, law) in laws.iter().enumerate() {
        let mut null = [0; N];
        for ((j, _), k) in chunks.iter().zip(&counts) {
            if *j == i {
                null.iter_mut().zip(k).for_each(|(n, k)| *n += k);
            }
        }
        let reps = (BATCH * batches(law)) as usize;
        let cells: Vec<String> = (null.iter())
            .map(|&k| {
                let e = Estimate::of(f64::from(k), reps);
                holds &= e.holds(LEVEL);
                format!("{k}, {:.5} ({:.5})", e.mean, e.upper)
            })
            .collect();
        println!("{} | {reps} | {}", law.name(), cells.join(" | "));
    }
    assert!(
        holds,
        "a candidate's null share has an upper end above the level"
    );
}

fn cell(n: Option<usize>) -> String {
    n.map_or("none".into(), |n| n.to_string())
}

#[test]
#[ignore = "long: 4,000 power replicates of 16,384 clusters for each of 18 scenarios; prints and asserts nothing; run in release mode"]
fn the_clusters_needed_are_printed() {
    const BATCHES: u32 = 4;
    let scenarios = grid();
    let results = par_map(&scenarios, |s| {
        (0..BATCHES)
            .map(|b| passes(s, BATCH, HORIZON, seed(&format!("power/{}/{b}", s.name()))))
            .collect::<Vec<_>>()
    });
    println!(
        "power: clusters needed, pooled over {BATCHES} batches of {BATCH} [lowest to highest batch]"
    );
    println!(
        "scenario | target | {} | best | ratio to the best",
        NAMES.join(" | ")
    );
    // Per candidate: its largest ratio to the best count over the cells it reaches, and the
    // cells it never reaches while another candidate does.
    let (mut worst, mut out, mut unreached) = ([1.0f64; N], [0u32; N], 0);
    for (s, batches) in scenarios.iter().zip(&results) {
        let pooled = pool(batches);
        for &t in &TARGETS {
            let count: [Option<usize>; N] =
                std::array::from_fn(|c| needed(&pooled[c], BATCH * BATCHES, t));
            let best = count.iter().flatten().min().copied();
            unreached += u32::from(best.is_none());
            let (mut cells, mut ratios) = (Vec::new(), Vec::new());
            for c in 0..N {
                let each = || batches.iter().map(|b| needed(&b[c], BATCH, t));
                let last = |n: &Option<usize>| n.unwrap_or(usize::MAX);
                cells.push(format!(
                    "{} [{} to {}]",
                    cell(count[c]),
                    cell(each().min_by_key(last).unwrap()),
                    cell(each().max_by_key(last).unwrap())
                ));
                ratios.push(match (count[c], best) {
                    (Some(n), Some(b)) => {
                        let r = n as f64 / b as f64;
                        worst[c] = worst[c].max(r);
                        format!("{r:.3}")
                    }
                    (None, Some(_)) => {
                        out[c] += 1;
                        "out".into()
                    }
                    _ => "-".into(),
                });
            }
            println!(
                "{} | {t} | {} | {} | {}",
                s.name(),
                cells.join(" | "),
                cell(best),
                ratios.join(" | ")
            );
        }
    }
    println!(
        "candidate | largest ratio to the best, over the cells it reaches | cells it never reaches while another does"
    );
    for c in 0..N {
        println!("{} | {:.3} | {}", NAMES[c], worst[c], out[c]);
    }
    println!("cells no candidate reaches within {HORIZON} clusters: {unreached}");
}
