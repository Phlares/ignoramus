//! Long simulations of a design with a blinded re-size from a reserve, ignored by default:
//! `cargo test -p evidence --release --test staged_level -- --ignored --nocapture`.
//!
//! Every scenario simulates one stratum of a two-stratum design, so its re-size asks that
//! stratum's share of power 0.8, 0.8^(1/2): a first batch of 21 clusters of 4 units and a
//! reserve, a mean claim below ln 1.04 on [-ln 2, ln 2] at level 0.005 (or 0.05, a level ten
//! times looser, where the sd 0.15 scenario rejects at all), sized at ln 1.02. The level tests
//! assert a bound on the pass share; the power test asserts nothing and prints its shares.
//! Each also prints the share that crossed 1 / level at some observation, which asserts
//! nothing.
//!
//! Shapes: the unit model draws normal units. A two-point null draws each cluster's observation
//! as `low` or `high`, with chance (MARGIN - low) / (high - low) of `high`, so its mean is the
//! margin: the ends of the range, or `high` at the top of the range and `low` at MARGIN - a
//! (ln 2 - MARGIN), a skew of a. On this range the top scores 0.893, where the cap acts most.
//!
//! Seeds: batch b of scenario `name` in the level tests (`level`) or the power test (`power`)
//! runs on `seed("level/<name>/<b>")` or `seed("power/<name>/<b>")`, four splitmix64 outputs
//! from the FNV-1a hash of that text, so each scenario and batch has its own seed.
//! A scenario runs in batches of 4,000 replicates, the count the registration gate uses: one
//! call of 100,000 would size its re-size table with 100,000 replicates per search, past
//! `WORK`.
use evidence::Tail;
use evidence::claim::{Bound, Statistic};
use evidence::power::{Shares, Staged, StagedStratum, staged_detail, staged_detail_by};
use std::f64::consts::LN_2;
use std::thread;
use std::time::Instant;

/// ln 1.04.
const MARGIN: f64 = 0.039221;
/// ln 1.02.
const POWER_AT: f64 = 0.019803;
const LEVEL: f64 = 0.005;
/// A level ten times looser than LEVEL; at LEVEL the sd 0.15 scenario almost never rejects.
const LOOSE_LEVEL: f64 = 0.05;
/// One stratum's share of a two-stratum claim at power 0.8: 0.8^(1/2).
const TARGET_EACH: f64 = 0.894427;
const BATCH: u32 = 4_000;
const BATCHES: u32 = 25;
const REPS: u32 = BATCH * BATCHES;

fn seed(text: &str) -> [u8; 32] {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut out = [0u8; 32];
    for chunk in out.chunks_mut(8) {
        h = h.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = h;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        chunk.copy_from_slice(&(z ^ (z >> 31)).to_le_bytes());
    }
    out
}

#[derive(Clone, Copy)]
enum Shape {
    Normal,
    TwoPoint { low: f64, high: f64 },
}

struct Scenario {
    name: String,
    reserve: usize,
    sd: f64,
    icc: f64,
    unit_mean: f64,
    level: f64,
    shape: Shape,
}

fn design(s: &Scenario) -> Staged {
    Staged {
        stat: Statistic::Mean {
            lo: -LN_2,
            hi: LN_2,
        },
        bounds: vec![Bound {
            tail: Tail::Lower,
            margin: MARGIN,
        }],
        strata: vec![StagedStratum {
            first: vec![4; 21],
            reserve: vec![4; s.reserve],
            sd: s.sd,
            icc: s.icc,
            unit_mean: s.unit_mean,
            size_at: POWER_AT,
        }],
        level: s.level,
        target_each: TARGET_EACH,
    }
}

/// The chance of `high` that gives a two-point null on `low` and `high` the mean MARGIN.
fn chance_of_high(low: f64, high: f64) -> f64 {
    (MARGIN - low) / (high - low)
}

/// A two-point null on `low` and `high`. The scenario declares the two points' sd, with an icc
/// of 1, so the re-size table centres on the spread the data have.
fn two_point(name: &str, low: f64, high: f64, level: f64) -> Scenario {
    let p = chance_of_high(low, high);
    Scenario {
        name: format!("level {level}, {name}"),
        reserve: 2_500,
        sd: (high - low) * (p * (1.0 - p)).sqrt(),
        icc: 1.0,
        unit_mean: MARGIN,
        level,
        shape: Shape::TwoPoint { low, high },
    }
}

/// The replicates that pass, stop and cross, out of [`REPS`], and the seconds they took.
fn counts(test: &str, s: &Scenario) -> ([u32; 3], f64) {
    let start = Instant::now();
    let d = design(s);
    let mut n = [0; 3];
    for b in 0..BATCHES {
        let seed = seed(&format!("{test}/{}/{b}", s.name));
        let shares = match s.shape {
            Shape::Normal => staged_detail(&d, BATCH, seed),
            Shape::TwoPoint { low, high } => {
                let p = chance_of_high(low, high);
                staged_detail_by(
                    &d,
                    BATCH,
                    seed,
                    |r| if r.uniform() <= p { high } else { low },
                )
            }
        };
        let Shares {
            pass,
            stop,
            crossed,
        } = shares.unwrap();
        for (k, x) in n.iter_mut().zip([pass, stop, crossed]) {
            assert!(x.is_finite(), "{x}");
            *k += (x * f64::from(BATCH)).round() as u32;
        }
    }
    (n, start.elapsed().as_secs_f64())
}

/// Runs every scenario on its own thread and prints a line per scenario: the pass, stop and
/// cross shares, each with its Monte Carlo standard error.
fn run(test: &str, scenarios: &[Scenario]) -> Vec<[u32; 3]> {
    let results: Vec<_> = thread::scope(|scope| {
        let handles: Vec<_> = (scenarios.iter())
            .map(|s| scope.spawn(move || counts(test, s)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let n = f64::from(REPS);
    let cell = |k: u32| {
        let p = f64::from(k) / n;
        format!("{p:.5} ({:.5})", (p * (1.0 - p) / n).sqrt())
    };
    println!("{test}: {REPS} replicates per scenario");
    println!("scenario | pass share (se) | stop share (se) | crossed share (se) | seconds");
    for (s, &([pass, stop, crossed], secs)) in scenarios.iter().zip(&results) {
        println!(
            "{} | {} | {} | {} | {secs:.0}",
            s.name,
            cell(pass),
            cell(stop),
            cell(crossed)
        );
    }
    results.into_iter().map(|(n, _)| n).collect()
}

/// Runs the null scenarios and asserts each pass share stays within three standard errors of
/// its level.
fn assert_level(scenarios: &[Scenario]) {
    let results = run("level", scenarios);
    for (s, [pass, ..]) in scenarios.iter().zip(results) {
        let bound = s.level + 3.0 * (s.level * (1.0 - s.level) / f64::from(REPS)).sqrt();
        let share = f64::from(pass) / f64::from(REPS);
        println!("{}: share {share:.5}, bound {bound:.7}", s.name);
        assert!(share <= bound, "{}: {share} > {bound}", s.name);
    }
}

#[test]
#[ignore = "long: about 100,000 replicates per scenario; run in release mode"]
fn staged_design_keeps_its_level() {
    let mut scenarios = Vec::new();
    for reserve in [0, 500, 2_500] {
        for sd in [0.03, 0.15] {
            for icc in [0.1, 0.8] {
                scenarios.push(Scenario {
                    name: format!("reserve {reserve}, sd {sd}, icc {icc}"),
                    reserve,
                    sd,
                    icc,
                    unit_mean: MARGIN,
                    level: LEVEL,
                    shape: Shape::Normal,
                });
            }
        }
    }
    assert_level(&scenarios);
}

#[test]
#[ignore = "long: about 100,000 replicates per scenario; run in release mode"]
fn staged_design_keeps_a_level_it_can_reach() {
    let scenarios: Vec<_> = [0.03, 0.15]
        .into_iter()
        .map(|sd| Scenario {
            name: format!("level {LOOSE_LEVEL}, reserve 2500, sd {sd}, icc 0.1"),
            reserve: 2_500,
            sd,
            icc: 0.1,
            unit_mean: MARGIN,
            level: LOOSE_LEVEL,
            shape: Shape::Normal,
        })
        .collect();
    assert_level(&scenarios);
}

#[test]
#[ignore = "long: about 100,000 replicates per scenario; run in release mode"]
fn staged_design_keeps_its_level_under_two_point_nulls() {
    let mut scenarios = Vec::new();
    for level in [LEVEL, LOOSE_LEVEL] {
        scenarios.push(two_point("ends", -LN_2, LN_2, level));
        for a in [0.03, 0.3] {
            let low = MARGIN - a * (LN_2 - MARGIN);
            scenarios.push(two_point(&format!("skew {a}"), low, LN_2, level));
        }
    }
    assert_level(&scenarios);
}

#[test]
#[ignore = "long: about 100,000 replicates per scenario; run in release mode"]
fn staged_design_power_at_the_pilot_and_a_third() {
    let scenarios: Vec<_> = [0.176, 0.176 / 3.0]
        .into_iter()
        .map(|sd| Scenario {
            name: format!("sd {sd:.4}, icc 0.82"),
            reserve: 2_500,
            sd,
            icc: 0.82,
            unit_mean: POWER_AT,
            level: LEVEL,
            shape: Shape::Normal,
        })
        .collect();
    run("power", &scenarios);
}
