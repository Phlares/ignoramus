//! Long simulations of a design with a blinded re-size from a reserve, ignored by default:
//! `cargo test -p evidence --release --test staged_level -- --ignored --nocapture`.
//!
//! Every scenario simulates one stratum of a two-stratum design, so its re-size asks that
//! stratum's share of power 0.8, 0.8^(1/2): a first batch of 21 clusters of 4 units and a
//! reserve, a mean claim below ln 1.04 on [-ln 2, ln 2] at level 0.005 (or 0.05, a level ten
//! times looser, where the sd 0.15 scenario rejects at all), sized at ln 1.02. The level tests
//! assert a bound on the pass share; the power test asserts nothing and prints its shares.
//!
//! Seeds: batch b of scenario `name` in the level tests (`level`) or the power test (`power`)
//! runs on `seed("level/<name>/<b>")` or `seed("power/<name>/<b>")`, four splitmix64 outputs
//! from the FNV-1a hash of that text, so each scenario and batch has its own seed.
//! A scenario runs in batches of 4,000 replicates, the count the registration gate uses: one
//! call of 100,000 would size its re-size table with 100,000 replicates per search, past
//! `WORK`.
use evidence::Tail;
use evidence::claim::{Bound, Statistic};
use evidence::power::{Staged, StagedStratum, staged_detail};
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

struct Scenario {
    name: String,
    reserve: usize,
    sd: f64,
    icc: f64,
    unit_mean: f64,
    level: f64,
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

/// The replicates that pass and that stop, out of [`REPS`], and the seconds they took.
fn counts(test: &str, s: &Scenario) -> (u32, u32, f64) {
    let start = Instant::now();
    let d = design(s);
    let (mut pass, mut stop) = (0, 0);
    for b in 0..BATCHES {
        let (p, q) = staged_detail(&d, BATCH, seed(&format!("{test}/{}/{b}", s.name))).unwrap();
        assert!(p.is_finite() && q.is_finite(), "{p} {q}");
        pass += (p * f64::from(BATCH)).round() as u32;
        stop += (q * f64::from(BATCH)).round() as u32;
    }
    (pass, stop, start.elapsed().as_secs_f64())
}

/// Runs every scenario on its own thread and prints a line per scenario: the pass and stop
/// shares, each with its Monte Carlo standard error.
fn run(test: &str, scenarios: &[Scenario]) -> Vec<(u32, u32)> {
    let results: Vec<_> = thread::scope(|scope| {
        let handles: Vec<_> = (scenarios.iter())
            .map(|s| scope.spawn(move || counts(test, s)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let n = f64::from(REPS);
    let se = |p: f64| (p * (1.0 - p) / n).sqrt();
    println!("{test}: {REPS} replicates per scenario");
    println!("scenario | pass share (se) | stop share (se) | seconds");
    for (s, &(pass, stop, secs)) in scenarios.iter().zip(&results) {
        let (p, q) = (f64::from(pass) / n, f64::from(stop) / n);
        println!(
            "{} | {p:.5} ({:.5}) | {q:.5} ({:.5}) | {secs:.0}",
            s.name,
            se(p),
            se(q)
        );
    }
    results.into_iter().map(|(p, q, _)| (p, q)).collect()
}

/// Runs the null scenarios and asserts each pass share stays within three standard errors of
/// its level.
fn assert_level(scenarios: &[Scenario]) {
    let results = run("level", scenarios);
    for (s, (pass, _)) in scenarios.iter().zip(results) {
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
        })
        .collect();
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
        })
        .collect();
    run("power", &scenarios);
}
