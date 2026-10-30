use evidence::claim::{Bound, Statistic};
use evidence::power::{Plan, Stratum, WORK, clusters_needed, mean_sd, simulate, work};
use evidence::rng::Rng;
use evidence::{Error, Tail};

const REPS: u32 = 2000;
const SEED: [u8; 32] = [11; 32];

fn below(margin: f64) -> Bound {
    Bound {
        tail: Tail::Lower,
        margin,
    }
}

fn above(margin: f64) -> Bound {
    Bound {
        tail: Tail::Upper,
        margin,
    }
}

/// `n` clusters of 4 units around -1, unit sd 1.
fn stratum(n: usize, icc: f64) -> Stratum {
    Stratum {
        sizes: vec![4; n],
        sd: 1.0,
        icc,
        unit_mean: -1.0,
    }
}

/// A mean below 0, cluster means in [-3, 3], at level 0.05.
fn plan(strata: Vec<Stratum>) -> Plan {
    Plan {
        stat: Statistic::Mean { lo: -3.0, hi: 3.0 },
        bounds: vec![below(0.0)],
        strata,
        level: 0.05,
    }
}

fn power(p: &Plan) -> f64 {
    simulate(p, REPS, &mut Rng::from_seed(SEED)).unwrap()
}

#[test]
fn power_rises_with_clusters_and_falls_with_icc() {
    let few = power(&plan(vec![stratum(20, 0.0)]));
    let more = power(&plan(vec![stratum(24, 0.0)]));
    let clustered = power(&plan(vec![stratum(24, 0.5)]));
    assert!(0.1 < few && few + 0.2 < more && more < 0.99, "{few} {more}");
    assert!(clustered + 0.1 < more, "{clustered} {more}");
}

#[test]
fn every_stratum_and_every_bound_must_pass() {
    // Two independent strata pass together about as often as the square of one alone.
    let one = power(&plan(vec![stratum(24, 0.0)]));
    let two = power(&plan(vec![stratum(24, 0.0), stratum(24, 0.0)]));
    assert!(0.3 < one && one < 0.95, "{one}");
    assert!((two - one * one).abs() < 0.04, "{one} {two}");
    // A within claim needs both of its bounds; each alone passes more often. The range is
    // centred on -1, so each bound's score has the same scale.
    let with = |bounds: Vec<Bound>| {
        power(&Plan {
            stat: Statistic::Mean { lo: -3.0, hi: 1.0 },
            bounds,
            ..plan(vec![stratum(22, 0.0)])
        })
    };
    let lower = with(vec![below(0.0)]);
    let upper = with(vec![above(-2.0)]);
    let both = with(vec![above(-2.0), below(0.0)]);
    assert!(both + 0.2 < lower.min(upper), "{lower} {upper} {both}");
}

#[test]
fn the_cluster_mean_has_the_variance_of_its_own_size() {
    // With icc 0, a cluster of 16 units with unit sd 2 has the mean sd of a cluster of 4
    // with unit sd 1, so one seed draws the same cluster means and the same power; one
    // cluster of 64 and 23 of 1 do not, though their mean size is about 3.6.
    let of = |sizes: Vec<u32>, sd: f64, icc: f64| {
        power(&plan(vec![Stratum {
            sizes,
            sd,
            icc,
            unit_mean: -1.0,
        }]))
    };
    let small = of(vec![4; 24], 1.0, 0.0);
    assert_eq!(small.to_bits(), of(vec![16; 24], 2.0, 0.0).to_bits());
    let mut mixed = vec![1; 23];
    mixed.push(64);
    assert!(of(mixed, 1.0, 0.0) + 0.2 < small);
    // sd^2 (icc + (1 - icc) / m): icc 0.5, sd 1 and 4 units give 0.625, as icc 0, sd
    // sqrt(0.625) and 1 unit do.
    let clustered = of(vec![4; 24], 1.0, 0.5);
    assert_eq!(
        clustered.to_bits(),
        of(vec![1; 24], 0.625f64.sqrt(), 0.0).to_bits()
    );
    assert!(clustered + 0.1 < small, "{clustered} {small}");
}

#[test]
fn mean_sd_averages_the_variance_of_each_cluster_mean() {
    let close = |a: f64, b: f64| assert!((a - b).abs() < 1e-12, "{a} != {b}");
    close(mean_sd(1.0, 0.5, &[4]), 0.625f64.sqrt());
    close(mean_sd(2.0, 0.0, &[16]), 0.5);
    close(mean_sd(3.0, 1.0, &[1, 100]), 3.0);
    // Over clusters of 1 and 4 units: sd^2 (0.2 + 0.8 (1 + 1/4) / 2) = 4 x 0.7.
    close(mean_sd(2.0, 0.2, &[1, 4]), 2.8f64.sqrt());
}

#[test]
fn a_quantile_claim_is_sized_on_the_share_of_units() {
    // The median of the units lies at -1, below the margin 0: about 84% of units are at or
    // below it, and more clusters give more power.
    let median = |n: usize| {
        power(&Plan {
            stat: Statistic::Quantile(0.5),
            ..plan(vec![stratum(n, 0.0)])
        })
    };
    let (few, more) = (median(8), median(24));
    assert!(few + 0.3 < more, "{few} {more}");
    // A unit model centred on the margin gives the claim nothing to find.
    let null = power(&Plan {
        stat: Statistic::Quantile(0.5),
        strata: vec![Stratum {
            unit_mean: 0.0,
            ..stratum(24, 0.0)
        }],
        ..plan(vec![])
    });
    assert!(null < 0.01, "{null}");
}

#[test]
fn the_simulation_is_fixed_by_its_seed() {
    let p = plan(vec![stratum(20, 0.0)]);
    let a = simulate(&p, REPS, &mut Rng::from_seed(SEED)).unwrap();
    let b = simulate(&p, REPS, &mut Rng::from_seed(SEED)).unwrap();
    let other = simulate(&p, REPS, &mut Rng::from_seed([12; 32])).unwrap();
    assert_eq!(a.to_bits(), b.to_bits());
    assert_ne!(a, other);
}

/// `p` with `n` clusters in each stratum: its declared sizes, repeated in order.
fn trial(p: &Plan, n: usize) -> Plan {
    Plan {
        strata: (p.strata.iter())
            .map(|s| Stratum {
                sizes: s.sizes.iter().copied().cycle().take(n).collect(),
                ..s.clone()
            })
            .collect(),
        ..p.clone()
    }
}

#[test]
fn clusters_needed_is_the_smallest_passing_count() {
    let p = plan(vec![stratum(3, 0.0), stratum(5, 0.0)]);
    let n = clusters_needed(&p, 0.8, REPS, SEED, 1024).unwrap().unwrap();
    assert!((20..60).contains(&n), "{n}");
    let at = |k: u32| power(&trial(&p, k as usize));
    assert!(at(n) >= 0.8, "{n}: {}", at(n));
    assert!(at(n - 1) < 0.8, "{n}: {}", at(n - 1));
    // The search ends at the cap.
    assert_eq!(clusters_needed(&p, 0.8, REPS, SEED, n), Ok(Some(n)));
    assert_eq!(clusters_needed(&p, 0.8, REPS, SEED, n - 1), Ok(None));
    assert_eq!(clusters_needed(&p, 0.8, REPS, SEED, 0), Ok(None));
    // A target of 0 is met by one cluster; one above 1 by none.
    assert_eq!(clusters_needed(&p, 0.0, REPS, SEED, 1024), Ok(Some(1)));
    assert_eq!(clusters_needed(&p, 1.01, REPS, SEED, 64), Ok(None));
}

#[test]
fn new_clusters_repeat_the_declared_sizes() {
    // 15 clusters of 1 unit, then 15 of 16, 0.5 below the margin at level 0.005. New clusters
    // repeat the declared sizes in order, so the trial at the present 30 is the present plan.
    let p = Plan {
        strata: vec![Stratum {
            sizes: [vec![1; 15], vec![16; 15]].concat(),
            sd: 1.0,
            icc: 0.0,
            unit_mean: -0.5,
        }],
        level: 0.005,
        ..plan(vec![])
    };
    let n = clusters_needed(&p, 0.8, REPS, SEED, 1024).unwrap().unwrap();
    let at = |k: u32| power(&trial(&p, k as usize));
    assert!(power(&p) < 0.8 && n > 30, "{n}");
    assert!(at(n) >= 0.8, "{n}: {}", at(n));
    assert!(at(n - 1) < 0.8, "{n}: {}", at(n - 1));
    // Sizes that repeat to the same clusters give the same count.
    let twice = Plan {
        strata: vec![Stratum {
            sizes: [vec![1; 15], vec![16; 15], vec![1; 15], vec![16; 15]].concat(),
            ..p.strata[0].clone()
        }],
        ..p.clone()
    };
    assert_eq!(clusters_needed(&twice, 0.8, REPS, SEED, 1024), Ok(Some(n)));
}

#[test]
fn a_plan_over_the_work_budget_is_refused() {
    // A mean draws one value per cluster; a quantile one per cluster and one per unit.
    let median = |sizes: Vec<u32>| Plan {
        stat: Statistic::Quantile(0.5),
        strata: vec![Stratum {
            sizes,
            ..stratum(0, 0.0)
        }],
        ..plan(vec![])
    };
    assert_eq!(work(&plan(vec![stratum(24, 0.0)]), REPS), 24 * 2000);
    assert_eq!(work(&median(vec![4; 24]), REPS), 24 * 5 * 2000);
    assert_eq!(WORK, 100_000_000);
    // 30 clusters of 1,000,000 units are refused before any draw.
    let big = median(vec![1_000_000; 30]);
    let mut rng = Rng::from_seed(SEED);
    assert_eq!(
        simulate(&big, REPS, &mut rng),
        Err(Error::Parameter("work"))
    );
    assert_eq!(
        clusters_needed(&big, 0.8, REPS, SEED, 1024),
        Err(Error::Parameter("work"))
    );
    // At the budget a plan runs; one value more, it does not.
    let edge = median(vec![99]);
    assert_eq!(work(&edge, 1_000_000), WORK);
    assert!(simulate(&edge, 1_000_000, &mut rng).is_ok());
    assert_eq!(
        simulate(&median(vec![100]), 1_000_000, &mut rng),
        Err(Error::Parameter("work"))
    );
    // The search counts every trial: 1 cluster of 20,000 units draws 40 million values, and
    // 2 clusters 80 million, each within the budget but 120 million in all.
    let wide = median(vec![19_999]);
    assert_eq!(clusters_needed(&wide, 0.8, REPS, SEED, 1), Ok(None));
    assert_eq!(
        clusters_needed(&wide, 0.8, REPS, SEED, 2),
        Err(Error::Parameter("work"))
    );
}

#[test]
fn a_bad_plan_is_refused() {
    let p = plan(vec![stratum(5, 0.0)]);
    let check = |edit: &dyn Fn(&mut Plan), name: &'static str| {
        let mut bad = p.clone();
        edit(&mut bad);
        let mut rng = Rng::from_seed(SEED);
        assert_eq!(simulate(&bad, REPS, &mut rng), Err(Error::Parameter(name)));
        assert_eq!(
            clusters_needed(&bad, 0.8, REPS, SEED, 8),
            Err(Error::Parameter(name))
        );
    };
    check(
        &|p| {
            p.bounds = vec![Bound {
                tail: Tail::Both,
                margin: 0.0,
            }]
        },
        "tail",
    );
    check(&|p| p.bounds = vec![below(3.0)], "bounds");
    check(&|p| p.bounds.clear(), "bounds");
    check(&|p| p.strata.clear(), "strata");
    check(&|p| p.strata[0].sizes.clear(), "sizes");
    check(&|p| p.strata[0].sizes[2] = 0, "sizes");
    for level in [0.0, 1.0, f64::NAN] {
        check(&|p| p.level = level, "level");
    }
    for icc in [-0.1, 1.1, f64::NAN] {
        check(&|p| p.strata[0].icc = icc, "icc");
    }
    for sd in [-1.0, f64::INFINITY, f64::NAN] {
        check(&|p| p.strata[0].sd = sd, "sd");
    }
    for mean in [f64::INFINITY, f64::NAN] {
        check(&|p| p.strata[0].unit_mean = mean, "unit_mean");
    }
    let mut rng = Rng::from_seed(SEED);
    assert_eq!(simulate(&p, 0, &mut rng), Err(Error::Parameter("reps")));
    assert_eq!(
        clusters_needed(&p, 0.8, 0, SEED, 8),
        Err(Error::Parameter("reps"))
    );
    // The edges are valid: icc 0 and 1, sd 0.
    for (icc, sd) in [(0.0, 1.0), (1.0, 1.0), (0.5, 0.0)] {
        let mut ok = p.clone();
        ok.strata[0].icc = icc;
        ok.strata[0].sd = sd;
        assert!(simulate(&ok, 10, &mut rng).is_ok(), "{icc} {sd}");
    }
}
