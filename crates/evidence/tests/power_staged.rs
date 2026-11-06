use evidence::claim::{Bound, Statistic};
use evidence::power::{
    CAP, Plan, Shares, Staged, StagedStratum, Stratum, WORK, blinded_sd, clusters_needed,
    resize_count, simulate, staged, staged_detail, staged_detail_by,
};
use evidence::rng::Rng;
use evidence::{Error, Tail};
use std::f64::consts::LN_2;

const REPS: u32 = 2000;
const SEED: [u8; 32] = [11; 32];

fn below(margin: f64) -> Bound {
    Bound {
        tail: Tail::Lower,
        margin,
    }
}

/// `first` and `reserve` clusters of 4 units around -1, unit sd 1, icc 0; sized at -1.
fn stratum(first: usize, reserve: usize) -> StagedStratum {
    StagedStratum {
        first: vec![4; first],
        reserve: vec![4; reserve],
        sd: 1.0,
        icc: 0.0,
        unit_mean: -1.0,
        size_at: -1.0,
    }
}

/// A mean below 0, cluster means in [-3, 3], at level 0.05; 0.8 asked of each stratum.
fn design(strata: Vec<StagedStratum>) -> Staged {
    Staged {
        stat: Statistic::Mean { lo: -3.0, hi: 3.0 },
        bounds: vec![below(0.0)],
        strata,
        level: 0.05,
        target_each: 0.8,
    }
}

/// The fixed-n plan of each stratum's first batch.
fn first_batch(d: &Staged) -> Plan {
    let strata = (d.strata.iter())
        .map(|s| Stratum {
            sizes: s.first.clone(),
            sd: s.sd,
            icc: s.icc,
            unit_mean: s.unit_mean,
        })
        .collect();
    Plan {
        stat: d.stat,
        bounds: d.bounds.clone(),
        strata,
        level: d.level,
    }
}

fn simulated(p: &Plan) -> f64 {
    simulate(p, REPS, &mut Rng::from_seed(SEED)).unwrap()
}

#[test]
fn the_limits_are_fixed() {
    assert_eq!(CAP, 16_384);
    assert_eq!(WORK, 2_000_000_000);
}

#[test]
fn blinded_sd_is_the_root_mean_square() {
    assert_eq!(blinded_sd(&[3.0, -4.0]), Some(12.5_f64.sqrt()));
    assert_eq!(blinded_sd(&[0.5; 7]), Some(0.5));
    assert_eq!(blinded_sd(&[]), None);
    assert_eq!(blinded_sd(&[f64::NAN]), None);
    assert_eq!(blinded_sd(&[1.0, f64::INFINITY]), None);
}

#[test]
fn blinded_sd_never_falls_below_the_centred_sd() {
    let mut rng = Rng::from_seed(SEED);
    for _ in 0..200 {
        let x: Vec<f64> = (0..21).map(|_| 0.1 + 0.5 * rng.normal()).collect();
        let mean = x.iter().sum::<f64>() / 21.0;
        let centred = (x.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / 21.0).sqrt();
        let blinded = blinded_sd(&x).unwrap();
        assert!(blinded >= centred, "{blinded} {centred}");
    }
}

/// A narrow claim: a mean below ln 1.04 on [-ln 2, ln 2] at level 0.005.
fn narrow() -> Plan {
    Plan {
        stat: Statistic::Mean {
            lo: -LN_2,
            hi: LN_2,
        },
        bounds: vec![below(0.039221)],
        strata: vec![],
        level: 0.005,
    }
}

#[test]
fn resize_count_grows_with_the_sd() {
    let sizes = vec![4; 2000];
    let count = |sizes: &[u32], s: f64| {
        resize_count(&narrow(), sizes, s, 0.019803, 0.8, REPS, SEED, 2000)
            .unwrap()
            .unwrap()
    };
    let n: Vec<u32> = [0.01, 0.02, 0.04]
        .iter()
        .map(|&s| count(&sizes, s))
        .collect();
    assert!(n[0] <= n[1] && n[1] <= n[2] && n[0] < n[2], "{n:?}");
    // It is `clusters_needed` on the one stratum, whose icc of 1 makes the units of a
    // cluster count for nothing.
    let one = Plan {
        strata: vec![Stratum {
            sizes: sizes.clone(),
            sd: 0.02,
            icc: 1.0,
            unit_mean: 0.019803,
        }],
        ..narrow()
    };
    assert_eq!(clusters_needed(&one, 0.8, REPS, SEED, 2000), Ok(Some(n[1])));
    assert_eq!(count(&[1; 2000], 0.02), n[1]);
}

#[test]
fn resize_count_is_none_past_the_cap() {
    let sizes = vec![4; 30];
    let count = |s: f64| resize_count(&narrow(), &sizes, s, 0.019803, 0.8, REPS, SEED, 30);
    assert_eq!(count(100.0), Ok(None));
    assert_eq!(count(-1.0), Err(Error::Parameter("sd")));
}

/// `first` clusters sized at `size_at`, with no reserve.
fn no_reserve(first: usize, size_at: f64) -> Staged {
    design(vec![StagedStratum {
        size_at,
        ..stratum(first, 0)
    }])
}

// With one stratum and no reserve, a replicate draws what `simulate`'s replicate draws, so it
// passes only when that one passes and the re-size does not stop.

#[test]
fn staged_with_no_reserve_equals_simulate() {
    // Sized at -2, the table's count for every s the first batch gives is at most 24.
    let d = no_reserve(24, -2.0);
    let fixed = simulated(&first_batch(&d));
    let Shares { pass, stop, .. } = staged_detail(&d, REPS, SEED).unwrap();
    assert!(fixed > 0.8, "{fixed}");
    assert!((pass - fixed).abs() <= 0.02, "{pass} {fixed}");
    assert_eq!(stop, 0.0);
    assert_eq!(staged(&d, REPS, SEED), Ok(pass));
}

#[test]
fn staged_counts_stops_as_failures() {
    // Below power, sized at -1.5, a few replicates stop: each one fails.
    let d = no_reserve(16, -1.5);
    let fixed = simulated(&first_batch(&d));
    let Shares { pass, stop, .. } = staged_detail(&d, REPS, SEED).unwrap();
    assert!(fixed < 0.8 && 0.0 < stop && stop < 0.1, "{fixed} {stop}");
    assert!(
        pass <= fixed && fixed - stop <= pass,
        "{pass} {stop} {fixed}"
    );
}

#[test]
fn staged_detail_reports_the_stop_share() {
    // Sized at -1, s0 is sqrt(0.5^2 + 1), and 20 clusters fall short at every s the first
    // batch gives: every replicate stops.
    let d = no_reserve(20, -1.0);
    let all = staged_detail(&d, REPS, SEED).unwrap();
    assert_eq!((all.pass, all.stop, all.crossed), (0.0, 1.0, 0.0));
    let Shares { pass, stop, .. } = staged_detail(&no_reserve(12, -2.0), REPS, SEED).unwrap();
    assert!(pass == 0.0 && 0.0 < stop && stop < 1.0, "{pass} {stop}");
}

#[test]
fn a_reserve_lifts_a_short_first_batch() {
    let d = design(vec![stratum(10, 300)]);
    let fixed = simulated(&first_batch(&d));
    let Shares { pass, stop, .. } = staged_detail(&d, REPS, SEED).unwrap();
    assert!(
        fixed < 0.5 && pass > 0.8 && stop < 0.01,
        "{fixed} {pass} {stop}"
    );
    // A study stops when any stratum stops.
    let Shares {
        pass: two,
        stop: stop_two,
        ..
    } = staged_detail(&design(vec![stratum(10, 300), stratum(10, 0)]), REPS, SEED).unwrap();
    assert!(stop_two > 0.5 && two < 0.5, "{two} {stop_two}");
}

#[test]
fn drawing_at_the_margin_rarely_passes() {
    let null = StagedStratum {
        unit_mean: 0.0,
        ..stratum(21, 300)
    };
    let d = Staged {
        level: 0.005,
        ..design(vec![null.clone()])
    };
    let Shares { pass, stop, .. } = staged_detail(&d, REPS, SEED).unwrap();
    assert!(pass <= 0.02 && stop < 1.0, "{pass} {stop}");
    // At level 0.5 some replicates reach 1 / level on the way and fall back below it by the end;
    // one that ends there reached it on the way.
    let loose = Staged {
        level: 0.5,
        ..design(vec![null])
    };
    let Shares { pass, crossed, .. } = staged_detail(&loose, REPS, SEED).unwrap();
    assert!(
        0.0 < pass && pass < crossed && crossed <= 0.5,
        "{pass} {crossed}"
    );
}

#[test]
fn draws_by_the_unit_model_match_staged_detail() {
    // icc 0, unit sd 1 and 4 units: each cluster mean has sd 0.5 about -1, one normal each.
    let d = design(vec![stratum(10, 300)]);
    let by = staged_detail_by(&d, REPS, SEED, |r| -1.0 + 0.5 * r.normal()).unwrap();
    assert_eq!(by, staged_detail(&d, REPS, SEED).unwrap());
}

#[test]
fn draws_by_a_closure_are_for_a_mean_only() {
    let d = Staged {
        stat: Statistic::Quantile(0.5),
        ..design(vec![stratum(5, 5)])
    };
    assert_eq!(
        staged_detail_by(&d, 10, SEED, |_| 0.0),
        Err(Error::Parameter("stat"))
    );
}

#[test]
fn draws_replace_the_data_and_not_the_table() {
    // With sd 0 the table's top value is 4 s0 = 4 x 0.5 (the_table_ends_at_four_times_s0).
    // Every cluster just past -2 gives s past it: every replicate stops. At -1 every one
    // continues, passes and crosses.
    let d = Staged {
        level: 0.5,
        target_each: 0.5,
        ..design(vec![StagedStratum {
            sd: 0.0,
            size_at: -0.5,
            ..stratum(5, 200)
        }])
    };
    let at = |x: f64| staged_detail_by(&d, 200, SEED, |_| x).unwrap();
    let past = at(-2.000001);
    assert_eq!((past.pass, past.stop), (0.0, 1.0));
    let inside = at(-1.0);
    assert_eq!((inside.pass, inside.stop, inside.crossed), (1.0, 0.0, 1.0));
}

#[test]
fn the_table_ends_at_four_times_s0() {
    // With sd 0 every cluster mean is the unit mean, so s is exactly its size; s0 is
    // |size_at| = 0.5 and the table's top value 2.0. At 2.0 the study continues; past it, it
    // stops.
    let at = |unit_mean: f64| {
        let s = StagedStratum {
            sd: 0.0,
            unit_mean,
            size_at: -0.5,
            ..stratum(5, 200)
        };
        let d = Staged {
            level: 0.5,
            target_each: 0.5,
            ..design(vec![s])
        };
        let s = staged_detail(&d, 200, SEED).unwrap();
        (s.pass, s.stop)
    };
    assert_eq!(at(-2.0), (1.0, 0.0));
    assert_eq!(at(-2.000001), (0.0, 1.0));
}

#[test]
fn staged_is_deterministic() {
    let d = design(vec![stratum(10, 40)]);
    let p = staged(&d, REPS, SEED).unwrap();
    assert_eq!(staged(&d, REPS, SEED), Ok(p));
    assert!(0.05 < p && p < 0.95, "{p}");
    assert_ne!(staged(&d, REPS, [12; 32]), Ok(p));
}

#[test]
fn a_bad_staged_plan_is_refused() {
    let check = |edit: &dyn Fn(&mut Staged), reps: u32, name: &'static str| {
        let mut bad = design(vec![stratum(5, 5)]);
        edit(&mut bad);
        assert_eq!(staged(&bad, reps, SEED), Err(Error::Parameter(name)));
    };
    check(&|d| d.strata[0].first.clear(), REPS, "sizes");
    check(&|d| d.strata[0].reserve[2] = 0, REPS, "sizes");
    check(&|d| d.strata[0].sd = f64::NAN, REPS, "sd");
    check(&|d| d.strata[0].size_at = f64::INFINITY, REPS, "size_at");
    check(&|d| d.target_each = 1.5, REPS, "target_each");
    check(&|d| d.target_each = -0.1, REPS, "target_each");
    check(&|d| d.strata.clear(), REPS, "strata");
    check(&|_| {}, 0, "reps");
    // A quantile draws every unit. The replicates alone: 1,001 values per cluster, 2 clusters,
    // 1,000,000 replicates are 2,002 million values.
    let median = |d: &mut Staged, first: u32, reserve: u32| {
        d.stat = Statistic::Quantile(0.5);
        d.strata[0].first = vec![first];
        d.strata[0].reserve = vec![reserve];
    };
    check(&|d| median(d, 1000, 1000), 1_000_000, "work");
    // The replicates and the table share one budget: here the replicates take it all (1,000
    // values per cluster), and the table's first search would draw more.
    check(&|d| median(d, 999, 999), 1_000_000, "work");
}
