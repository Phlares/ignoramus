//! Simulations for the choice of the bet cap (decision 0022, "Open"). The two long ones are
//! ignored by default:
//! `cargo test -p evidence --release --test bet_cap -- --ignored --nocapture`.
//!
//! Score units: a "mean below 0" claim on [-1, 1] at level 0.005, so an observation is its own
//! score. A scenario draws each cluster observation at the ceiling, +1, with chance `ceiling`,
//! and otherwise normal with sd `spread` about -`gap`, clipped to the range. Its null twin
//! moves the normal part so the mean of all observations is 0.
//!
//! Candidates: the caps 1/2, 3/4 and 0.9, and the mean of their three e-values, which is an
//! e-value too. Every candidate reads the same draws.
//!
//! Power: per candidate and target, the smallest count of clusters at which the share of
//! replicates with an e-value of at least 1 / level reaches the target. One path per replicate
//! serves every count, since a prefix of independent draws has the law of a shorter study.
//! Null: the share of replicates whose e-value reaches 1 / level at any count up to the
//! horizon. Ville's inequality bounds that share by the level, and it bounds every fixed count.
//! A candidate keeps the level when the upper end of the share's 95% interval is at most the
//! level. With ceiling units the share lies near the bound, so those nulls take 500,000
//! replicates; the others take 20,000.
//!
//! Seeds: batch b of scenario `name` runs on `seed("power/<name>/<b>")` or
//! `seed("null/<name>/<b>")`.
mod support;

use evidence::Tail;
use evidence::claim::{Bound, Statistic, e_value};
use evidence::eprocess::{EProcess, MAX_BET, Score};
use evidence::rng::Rng;
use evidence::sim::Estimate;
use std::thread;
use support::seed;

const LEVEL: f64 = 0.005;
const CAPS: [f64; 3] = [0.5, MAX_BET, 0.9];
const NAMES: [&str; 4] = ["1/2", "3/4", "0.9", "mixture"];
/// One stratum's share of a joint power of 0.8 over 1, 2 and 4 strata.
const TARGETS: [f64; 3] = [0.8, 0.894427, 0.945742];
/// The most clusters per stratum the power gate's search tries (`power::CAP`).
const HORIZON: usize = 16_384;
const SCORE: Score = Score::Bounded {
    lo: -1.0,
    hi: 1.0,
    mean: 0.0,
};

#[derive(Clone, Copy)]
struct Scenario {
    gap: f64,
    spread: f64,
    ceiling: f64,
}

impl Scenario {
    fn name(&self) -> String {
        format!(
            "gap {}, spread {}, ceiling {}",
            self.gap, self.spread, self.ceiling
        )
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

/// The four candidates over one path.
struct Candidates([EProcess; 3]);

impl Candidates {
    fn new() -> Candidates {
        Candidates(
            CAPS.map(|cap| EProcess::with_cap(SCORE, Tail::Lower, f64::INFINITY, cap).unwrap()),
        )
    }

    /// Each candidate's e-value after `x`.
    fn observe(&mut self, x: f64) -> [f64; 4] {
        let mut v = [0.0; 4];
        for (p, v) in self.0.iter_mut().zip(&mut v) {
            p.observe(x).unwrap();
            *v = p.value();
        }
        v[3] = (v[0] + v[1] + v[2]) / 3.0;
        v
    }
}

fn reaches(e: f64) -> bool {
    evidence::rejects(e, LEVEL)
}

/// `passed[c][n]`: the replicates in which candidate c reaches 1 / level after exactly n
/// observations about -`gap`.
fn passes(s: &Scenario, reps: u32, horizon: usize, seed: [u8; 32]) -> [Vec<u32>; 4] {
    let mut rng = Rng::from_seed(seed);
    let mut passed = [(); 4].map(|()| vec![0; horizon + 1]);
    for _ in 0..reps {
        let mut c = Candidates::new();
        for n in 1..=horizon {
            let v = c.observe(s.draw(-s.gap, &mut rng));
            for (p, v) in passed.iter_mut().zip(v) {
                p[n] += u32::from(reaches(v));
            }
        }
    }
    passed
}

/// The replicates of the null twin in which each candidate reaches 1 / level at any count up
/// to `horizon`.
fn crossed(s: &Scenario, reps: u32, horizon: usize, seed: [u8; 32]) -> [u32; 4] {
    let mut rng = Rng::from_seed(seed);
    let mut crossed = [0; 4];
    for _ in 0..reps {
        let mut c = Candidates::new();
        let mut ever = [false; 4];
        for _ in 0..horizon {
            let v = c.observe(s.draw(s.null_mean(), &mut rng));
            for (e, v) in ever.iter_mut().zip(v) {
                *e |= reaches(v);
            }
        }
        for (k, e) in crossed.iter_mut().zip(ever) {
            *k += u32::from(e);
        }
    }
    crossed
}

/// The smallest count at which the pass share reaches `target`.
fn needed(passed: &[u32], reps: u32, target: f64) -> Option<usize> {
    (passed.iter()).position(|&k| f64::from(k) / f64::from(reps) >= target)
}

#[test]
fn the_cap_in_force_gives_the_claim_test() {
    let stat = Statistic::Mean { lo: -1.0, hi: 1.0 };
    let bound = Bound {
        tail: Tail::Lower,
        margin: 0.0,
    };
    let s = Scenario {
        gap: 0.03,
        spread: 0.3,
        ceiling: 0.01,
    };
    let mut rng = Rng::from_seed(seed("claim"));
    let obs: Vec<f64> = (0..400).map(|_| s.draw(-s.gap, &mut rng)).collect();
    let mut c = Candidates::new();
    let last = obs.iter().map(|&x| c.observe(x)).last().unwrap();
    assert_eq!(last[1], e_value(&stat, &bound, &obs).unwrap());
    assert!(last[0] != last[1] && last[2] != last[1], "{last:?}");
    assert_eq!(last[3], (last[0] + last[1] + last[2]) / 3.0);
}

#[test]
fn needed_is_the_first_count_at_the_target() {
    assert_eq!(needed(&[0, 1, 3, 2, 4], 4, 0.75), Some(2));
    assert_eq!(needed(&[0, 1, 3, 2, 4], 4, 1.0), Some(4));
    assert_eq!(needed(&[0, 1], 4, 0.5), None);
}

#[test]
fn the_null_twin_has_mean_zero() {
    let s = Scenario {
        gap: 0.03,
        spread: 0.02,
        ceiling: 0.01,
    };
    let mut rng = Rng::from_seed(seed("twin"));
    let n = 400_000;
    let (mut sum, mut top) = (0.0, 0);
    for _ in 0..n {
        let x = s.draw(s.null_mean(), &mut rng);
        sum += x;
        top += u32::from(x == 1.0);
    }
    // One draw has sd of about 0.1, so the mean has a standard error of about 0.00016.
    let mean = sum / f64::from(n);
    assert!(mean.abs() < 0.0008, "{mean}");
    let share = f64::from(top) / f64::from(n);
    assert!((share - s.ceiling).abs() < 0.001, "{share}");
}

/// Decision 0022 reports about 400 and 281 clusters at caps 1/2 and 3/4 for a claim with this
/// gap at power 0.894, from a simulation outside this repository.
#[test]
fn without_ceiling_units_a_higher_cap_needs_fewer_clusters() {
    let s = Scenario {
        gap: 0.0297,
        spread: 0.0266,
        ceiling: 0.0,
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

/// The gaps of the claims on file over their score scale, the spread of the finished study's
/// two strata over it, and decision 0022's ceiling shares.
fn grid() -> Vec<Scenario> {
    let mut out = Vec::new();
    for ceiling in [0.0, 0.005, 0.01] {
        for gap in [0.0142, 0.0220, 0.0297] {
            for spread in [0.0172, 0.0266] {
                out.push(Scenario {
                    gap,
                    spread,
                    ceiling,
                });
            }
        }
    }
    out
}

fn cell(n: Option<usize>) -> String {
    n.map_or("none".into(), |n| n.to_string())
}

type Counts = [Option<usize>; 4];

/// The null crossings of scenario `s` over `batches`, each of `batch` replicates.
fn nulls_of(s: &Scenario, batch: u32, batches: std::ops::Range<u32>) -> [u32; 4] {
    let mut null = [0; 4];
    for b in batches {
        let k = crossed(s, batch, HORIZON, seed(&format!("null/{}/{b}", s.name())));
        for (n, k) in null.iter_mut().zip(k) {
            *n += k;
        }
    }
    null
}

/// With ceiling units the null twins sit close to Ville's bound, and 20,000 replicates cannot
/// place a share below the level. Batches 0 to 19 are those of the test below.
#[test]
#[ignore = "long: 500,000 null replicates of 16,384 clusters for each of 12 scenarios; run in release mode"]
fn the_ceiling_nulls_keep_the_level_at_500_000_replicates() {
    const BATCH: u32 = 1_000;
    const CHUNK: u32 = 100;
    const CHUNKS: u32 = 5;
    let scenarios: Vec<Scenario> = grid().into_iter().filter(|s| s.ceiling > 0.0).collect();
    let results: Vec<[u32; 4]> = thread::scope(|scope| {
        let handles: Vec<Vec<_>> = (scenarios.iter())
            .map(|s| {
                (0..CHUNKS)
                    .map(|c| scope.spawn(move || nulls_of(s, BATCH, c * CHUNK..(c + 1) * CHUNK)))
                    .collect()
            })
            .collect();
        (handles.into_iter())
            .map(|chunks| {
                chunks.into_iter().fold([0; 4], |mut sum, h| {
                    for (n, k) in sum.iter_mut().zip(h.join().unwrap()) {
                        *n += k;
                    }
                    sum
                })
            })
            .collect()
    });
    let reps = (BATCH * CHUNK * CHUNKS) as usize;
    println!(
        "null: replicates of {reps} that reach 1 / {LEVEL} at any count up to {HORIZON}: count, share (upper end of its 95% interval)"
    );
    println!("scenario | {}", NAMES.join(" | "));
    let mut holds = true;
    for (s, null) in scenarios.iter().zip(&results) {
        let cells: Vec<String> = (null.iter())
            .map(|&k| {
                let e = Estimate::of(f64::from(k), reps);
                holds &= e.holds(LEVEL);
                format!("{k}, {:.5} ({:.5})", e.mean, e.upper)
            })
            .collect();
        println!("{} | {}", s.name(), cells.join(" | "));
    }
    assert!(
        holds,
        "a candidate's null share has an upper end above the level"
    );
}

/// One scenario: the null crossings, then per target the pooled counts and each batch's.
fn scenario(
    s: &Scenario,
    batch: u32,
    nulls: u32,
    powers: u32,
) -> ([u32; 4], Vec<(Counts, Vec<Counts>)>) {
    let null = nulls_of(s, batch, 0..nulls);
    let batches: Vec<_> = (0..powers)
        .map(|b| passes(s, batch, HORIZON, seed(&format!("power/{}/{b}", s.name()))))
        .collect();
    let per_target = (TARGETS.iter())
        .map(|&t| {
            let pooled = [0, 1, 2, 3].map(|c| {
                let sum: Vec<u32> = (0..=HORIZON)
                    .map(|n| batches.iter().map(|p| p[c][n]).sum())
                    .collect();
                needed(&sum, batch * powers, t)
            });
            let each = (batches.iter())
                .map(|p| [0, 1, 2, 3].map(|c| needed(&p[c], batch, t)))
                .collect();
            (pooled, each)
        })
        .collect();
    (null, per_target)
}

#[test]
#[ignore = "long: 4,000 power replicates of 16,384 clusters per scenario, and 20,000 null ones without ceiling units; run in release mode"]
fn every_candidate_keeps_the_level_and_the_counts_are_printed() {
    const BATCH: u32 = 1_000;
    const NULLS: u32 = 20;
    const POWERS: u32 = 4;
    // The nulls with ceiling units need the 500,000 replicates of the test above.
    let nulls = |s: &Scenario| if s.ceiling > 0.0 { 0 } else { NULLS };
    let scenarios = grid();
    let results: Vec<_> = thread::scope(|scope| {
        let handles: Vec<_> = (scenarios.iter())
            .map(|s| scope.spawn(move || scenario(s, BATCH, nulls(s), POWERS)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let null_reps = (BATCH * NULLS) as usize;
    println!(
        "null: share of {null_reps} replicates that reach 1 / {LEVEL} at any count up to {HORIZON} (upper end of its 95% interval)"
    );
    println!("scenario | {}", NAMES.join(" | "));
    let mut holds = true;
    for (s, (null, _)) in scenarios.iter().zip(&results) {
        if nulls(s) == 0 {
            continue;
        }
        let cells: Vec<String> = (null.iter())
            .map(|&k| {
                let e = Estimate::of(f64::from(k), null_reps);
                holds &= e.holds(LEVEL);
                format!("{:.5} ({:.5})", e.mean, e.upper)
            })
            .collect();
        println!("{} | {}", s.name(), cells.join(" | "));
    }

    println!(
        "power: clusters needed, pooled over {POWERS} batches of {BATCH} [lowest to highest batch]"
    );
    println!(
        "scenario | target | {} | best | ratio to the best",
        NAMES.join(" | ")
    );
    // Per candidate: its largest ratio to the best count, and the cells it never reaches while
    // another candidate does.
    let mut worst = [1.0f64; 4];
    let mut out = [0u32; 4];
    for (s, (_, per_target)) in scenarios.iter().zip(&results) {
        for (&t, (pooled, each)) in TARGETS.iter().zip(per_target) {
            let cells: Vec<String> = (0..4)
                .map(|c| {
                    let ends = each.iter().map(|b| b[c].unwrap_or(usize::MAX));
                    let end = |n: Option<usize>| cell(n.filter(|&n| n != usize::MAX));
                    format!(
                        "{} [{} to {}]",
                        cell(pooled[c]),
                        end(ends.clone().min()),
                        end(ends.max())
                    )
                })
                .collect();
            let best = pooled.iter().flatten().min().copied();
            let ratios: Vec<String> = (0..4)
                .map(|c| match (pooled[c], best) {
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
                })
                .collect();
            println!(
                "{} | {t} | {} | {} | {}",
                s.name(),
                cells.join(" | "),
                cell(best),
                ratios.join(" | ")
            );
        }
    }
    println!("candidate | largest ratio to the best | cells it never reaches while another does");
    for c in 0..4 {
        println!("{} | {:.3} | {}", NAMES[c], worst[c], out[c]);
    }
    assert!(
        holds,
        "a candidate's null share has an upper end above the level"
    );
}
