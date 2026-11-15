//! Null simulations of the claim test when clusters share a source above them. The long one
//! is ignored by default:
//! `cargo test -p evidence --release --test dependent_clusters -- --ignored --nocapture`.
//!
//! Score units: a "mean below 0" claim on [-1, 1]. A scenario has `clusters` clusters in
//! `groups` groups of equal size. A cluster observation is its group's shift plus its own
//! noise, both normal with mean 0; the shift holds `share` of the variance `sd`^2.
//!
//! Two nulls. `Population`: every replicate draws new shifts, so the law has mean 0 and the
//! set that was drawn does not. The test counts clusters as independent; this null measures
//! what a shared source does to its level for a claim about the population. `Set`: the
//! replicate's observations are moved so their own mean is 0. The claim is then about that
//! set, and only the order is left to chance.
//!
//! Two orders. `Seeded`: `claim::order`, as a study runs its clusters. `Grouped`: each group's
//! clusters in a row.
//!
//! Each line prints the share of replicates that pass (an e-value of at least 1 / level after
//! the last cluster) and the share that reach 1 / level at any cluster, at levels 0.005 and
//! 0.05, each with the upper end of its 95% interval. Only the independent control asserts.
//!
//! Seeds: a scenario runs on `seed("dependent/<name>")`.
mod support;

use evidence::Tail;
use evidence::claim::{Bound, Statistic, e_values, order};
use evidence::rng::Rng;
use evidence::sim::Estimate;
use std::thread;
use support::seed;

const LEVELS: [f64; 2] = [0.005, 0.05];
const STAT: Statistic = Statistic::Mean { lo: -1.0, hi: 1.0 };
const BOUND: Bound = Bound {
    tail: Tail::Lower,
    margin: 0.0,
};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Null {
    Population,
    Set,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Order {
    Seeded,
    Grouped,
}

#[derive(Debug, Clone, Copy)]
struct Scenario {
    clusters: usize,
    groups: usize,
    sd: f64,
    share: f64,
    null: Null,
    order: Order,
}

impl Scenario {
    fn name(&self) -> String {
        format!(
            "{:?}, {:?}, sd {}, {} groups of {}, share {}",
            self.null,
            self.order,
            self.sd,
            self.groups,
            self.clusters / self.groups,
            self.share
        )
    }

    /// One replicate's cluster observations in test order.
    fn draw(&self, rng: &mut Rng) -> Vec<f64> {
        let between = self.sd * self.share.sqrt();
        let within = self.sd * (1.0 - self.share).sqrt();
        let mut xs = Vec::with_capacity(self.clusters);
        for _ in 0..self.groups {
            let shift = between * rng.normal();
            let own = (0..self.clusters / self.groups).map(|_| shift + within * rng.normal());
            xs.extend(own.map(|x| x.clamp(-1.0, 1.0)));
        }
        if self.null == Null::Set {
            let mean = xs.iter().sum::<f64>() / xs.len() as f64;
            // The clamp can move a value at an end of the range; at these spreads none is near.
            xs.iter_mut()
                .for_each(|x| *x = (*x - mean).clamp(-1.0, 1.0));
        }
        match self.order {
            Order::Grouped => xs,
            Order::Seeded => order(xs.len(), rng).into_iter().map(|i| xs[i]).collect(),
        }
    }
}

/// Per level: the replicates that pass, and the replicates that reach 1 / level at any cluster.
fn counts(s: &Scenario, reps: u32) -> [[u32; 2]; 2] {
    let mut rng = Rng::from_seed(seed(&format!("dependent/{}", s.name())));
    let mut n = [[0; 2]; 2];
    for _ in 0..reps {
        let (last, peak) = e_values(&STAT, &BOUND, &s.draw(&mut rng)).unwrap();
        for (k, &level) in n.iter_mut().zip(&LEVELS) {
            k[0] += u32::from(evidence::rejects(last, level));
            k[1] += u32::from(evidence::rejects(peak, level));
        }
    }
    n
}

fn base(null: Null, order: Order) -> Scenario {
    Scenario {
        clusters: 320,
        groups: 4,
        sd: 0.25,
        share: 0.5,
        null,
        order,
    }
}

#[test]
fn clusters_of_a_group_share_its_shift() {
    let whole = Scenario {
        share: 1.0,
        ..base(Null::Population, Order::Grouped)
    };
    let xs = whole.draw(&mut Rng::from_seed(seed("shift")));
    assert_eq!(xs.len(), 320);
    for group in xs.chunks(80) {
        assert!(group.iter().all(|&x| x == group[0]), "{group:?}");
    }
    assert!(xs[0] != xs[80] && xs[80] != xs[160]);
    let none = Scenario {
        share: 0.0,
        ..whole
    };
    let xs = none.draw(&mut Rng::from_seed(seed("shift")));
    assert!(xs[0] != xs[1]);
}

#[test]
fn the_set_null_has_mean_zero_and_the_population_null_does_not() {
    let mean = |s: Scenario| {
        let xs = s.draw(&mut Rng::from_seed(seed("mean")));
        xs.iter().sum::<f64>() / xs.len() as f64
    };
    assert!(mean(base(Null::Set, Order::Seeded)).abs() < 1e-12);
    assert!(mean(base(Null::Set, Order::Grouped)).abs() < 1e-12);
    // Four shifts of sd 0.177: their mean has sd 0.088.
    assert!(mean(base(Null::Population, Order::Grouped)).abs() > 1e-3);
}

#[test]
fn the_seeded_order_moves_the_same_values() {
    let sorted = |s: Scenario| {
        let mut xs = s.draw(&mut Rng::from_seed(seed("order")));
        xs.sort_by(f64::total_cmp);
        xs
    };
    let grouped = base(Null::Population, Order::Grouped);
    let seeded = base(Null::Population, Order::Seeded);
    assert_eq!(sorted(grouped), sorted(seeded));
    let draw = |s: Scenario| s.draw(&mut Rng::from_seed(seed("order")));
    assert_ne!(draw(grouped), draw(seeded));
}

/// Four groups that share half the variance: the set's mean has sd 0.088, so about half the
/// population-null sets lie far enough below 0 for 320 clusters to pass.
#[test]
fn a_shared_source_breaks_the_level_for_the_population_and_not_for_the_set() {
    let reps = 2_000;
    let [_, [population, _]] = counts(&base(Null::Population, Order::Seeded), reps);
    let [_, [set, _]] = counts(&base(Null::Set, Order::Seeded), reps);
    assert!(
        f64::from(population) / f64::from(reps) > 0.2,
        "{population}"
    );
    assert!(f64::from(set) / f64::from(reps) <= 0.05, "{set}");
}

fn grid() -> Vec<Scenario> {
    let mut out = Vec::new();
    for null in [Null::Population, Null::Set] {
        for order in [Order::Seeded, Order::Grouped] {
            for sd in [0.0266, 0.25] {
                let s = Scenario {
                    sd,
                    ..base(null, order)
                };
                // The control: every cluster is its own group.
                out.push(Scenario {
                    groups: 320,
                    share: 0.0,
                    ..s
                });
                for groups in [64, 16, 4] {
                    for share in [0.1, 0.5] {
                        out.push(Scenario { groups, share, ..s });
                    }
                }
            }
        }
    }
    out
}

#[test]
#[ignore = "long: 100,000 replicates of 320 clusters per scenario; run in release mode"]
fn the_independent_control_keeps_its_level_and_every_share_is_printed() {
    const REPS: u32 = 100_000;
    let scenarios = grid();
    let results: Vec<_> = thread::scope(|scope| {
        let handles: Vec<_> = (scenarios.iter())
            .map(|s| scope.spawn(move || counts(s, REPS)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    println!("{REPS} replicates per scenario; share (upper end of its 95% interval)");
    println!("scenario | pass at 0.005 | reached at 0.005 | pass at 0.05 | reached at 0.05");
    for (s, n) in scenarios.iter().zip(&results) {
        let cells: Vec<String> = (n.iter().flatten())
            .map(|&k| {
                let e = Estimate::of(f64::from(k), REPS as usize);
                format!("{:.5} ({:.5})", e.mean, e.upper)
            })
            .collect();
        println!("{} | {}", s.name(), cells.join(" | "));
    }
    for (s, n) in scenarios.iter().zip(&results) {
        if s.groups == s.clusters && s.null == Null::Population {
            for (k, &level) in n.iter().zip(&LEVELS) {
                let reached = Estimate::of(f64::from(k[1]), REPS as usize);
                assert!(reached.holds(level), "{}: {reached:?}", s.name());
            }
        }
    }
}
