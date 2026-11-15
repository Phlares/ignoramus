use evidence::eprocess::{EProcess, MAX_BET, Score};
use evidence::rng::Rng;
use evidence::{Error, Tail};

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-12, "{a} != {b}");
}

fn run(p: &mut EProcess, xs: &[f64]) -> Vec<f64> {
    xs.iter()
        .map(|&x| {
            p.observe(x).unwrap();
            p.value()
        })
        .collect()
}

#[test]
fn sign_upper_matches_hand_values() {
    let mut p = EProcess::new(Score::Sign, Tail::Upper, f64::INFINITY).unwrap();
    let v = run(&mut p, &[1.0, 2.0, -1.0, 3.0]);
    // The third bet is 2/3, below the cap; under a cap of 1/2 it would be 1/2.
    for (a, b) in v.iter().zip([1.0, 1.5, 0.5, 0.625]) {
        close(*a, b);
    }
    close(p.p_value(), 1.0 / 1.5);
    close(p.peak(), 1.5);
    assert_eq!(p.observed(), 4);
    assert_eq!(MAX_BET, 0.75);
}

#[test]
fn a_score_at_the_far_end_never_zeroes_wealth() {
    // Scores of 1/2 push each side's bet past 1, so it bets the cap; then a score at the far
    // end meets it. A cap of 1 makes each log wealth -inf, and their mean NaN.
    const { assert!(MAX_BET < 1.0) };
    let mut p = EProcess::new(Score::Clip { scale: 1.0 }, Tail::Both, f64::INFINITY).unwrap();
    let xs: Vec<f64> = [vec![0.5; 10], vec![-1.0], vec![-0.5; 40], vec![1.0]].concat();
    for (i, v) in run(&mut p, &xs).into_iter().enumerate() {
        assert!(v.is_finite() && v > 0.0, "{i}: {v}");
    }
}

#[test]
fn sign_of_zero_is_zero() {
    let mut p = EProcess::new(Score::Sign, Tail::Upper, f64::INFINITY).unwrap();
    let v = run(&mut p, &[0.0, 1.0, 1.0]);
    for (a, b) in v.iter().zip([1.0, 1.0, 1.5]) {
        close(*a, b);
    }
}

#[test]
fn clip_matches_hand_values() {
    let mut p = EProcess::new(Score::Clip { scale: 2.0 }, Tail::Upper, f64::INFINITY).unwrap();
    let v = run(&mut p, &[1.0, 4.0, -1.0]);
    for (a, b) in v.iter().zip([1.0, 1.4, 1.4 * (1.0 - 1.0 / 3.0)]) {
        close(*a, b);
    }
}

#[test]
fn bounded_scores_against_the_null_mean() {
    let s = Score::Bounded {
        lo: 0.0,
        hi: 1.0,
        mean: 0.25,
    };
    let mut p = EProcess::new(s, Tail::Upper, f64::INFINITY).unwrap();
    let v = run(&mut p, &[1.0, 0.0]);
    close(v[0], 1.0);
    close(v[1], 1.0 + 0.5 * (-1.0 / 3.0));
}

#[test]
fn bounded_scale_is_the_longer_side_with_a_negative_lower_bound() {
    let s = Score::Bounded {
        lo: -1.0,
        hi: 1.0,
        mean: 0.5,
    };
    let mut p = EProcess::new(s, Tail::Upper, f64::INFINITY).unwrap();
    let v = run(&mut p, &[1.0, 1.0]);
    // D = 1.5, s = 0.5 / 1.5 = 1/3; lambda_2 = (1/3) / (1 + 1/9) = 0.3
    close(v[1], 1.0 + 0.3 / 3.0);
}

#[test]
fn lower_tail_bets_on_negative_scores() {
    let mut p = EProcess::new(Score::Sign, Tail::Lower, f64::INFINITY).unwrap();
    let v = run(&mut p, &[-1.0, -1.0]);
    close(v[1], 1.5);
}

#[test]
fn both_tails_average_the_two_wealths() {
    let mut p = EProcess::new(Score::Sign, Tail::Both, f64::INFINITY).unwrap();
    let v = run(&mut p, &[1.0, 1.0]);
    close(v[1], 1.25);
}

#[test]
fn stop_freezes_the_value() {
    let mut p = EProcess::new(Score::Sign, Tail::Upper, 1.4).unwrap();
    p.observe(1.0).unwrap();
    assert!(!p.stopped());
    p.observe(1.0).unwrap();
    assert!(p.stopped());
    p.observe(-1.0).unwrap();
    close(p.value(), 1.5);
    assert_eq!(p.observed(), 2);
    assert_eq!(p.observe(f64::NAN), Err(Error::NotFinite));
    assert_eq!(p.observe(f64::INFINITY), Err(Error::NotFinite));
    assert_eq!(p.observe(5.0), Ok(()));
    assert_eq!(p.observed(), 2);
    assert!(p.value() >= 1.4);

    let mut q = EProcess::new(Score::Sign, Tail::Upper, 1.5).unwrap();
    run(&mut q, &[1.0, 1.0]);
    assert!(q.stopped() && q.value() >= 1.5);
    let mut r = EProcess::new(Score::Sign, Tail::Upper, 1.6).unwrap();
    run(&mut r, &[1.0, 1.0]);
    assert!(!r.stopped());
}

#[test]
fn bet_is_set_before_the_observation() {
    let lambda_after = |last: f64| {
        let mut p = EProcess::new(Score::Clip { scale: 1.0 }, Tail::Upper, f64::INFINITY).unwrap();
        run(&mut p, &[0.5, -0.5, 0.5]);
        let old = p.value();
        p.observe(last).unwrap();
        (p.value() / old - 1.0) / last.clamp(-1.0, 1.0)
    };
    close(lambda_after(0.9), lambda_after(-0.3));
    close(lambda_after(0.9), 2.0 / 7.0);
}

#[test]
fn rejects_bad_parameters_and_observations() {
    let new = |s, st| EProcess::new(s, Tail::Upper, st).err();
    assert_eq!(new(Score::Sign, 1.0), Some(Error::Parameter("stop")));
    assert_eq!(new(Score::Sign, f64::NAN), Some(Error::Parameter("stop")));
    assert_eq!(new(Score::Sign, 0.5), Some(Error::Parameter("stop")));
    assert!(new(Score::Sign, 1.0 + 1e-9).is_none());
    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            new(Score::Clip { scale }, 2.0),
            Some(Error::Parameter("scale"))
        );
    }
    let b = |lo, hi, mean| new(Score::Bounded { lo, hi, mean }, 2.0);
    let bad = Some(Error::Parameter("bounds"));
    assert_eq!(b(1.0, 0.0, 0.5), bad);
    assert_eq!(b(0.0, 1.0, 0.0), bad);
    assert_eq!(b(0.0, 1.0, 1.0), bad);
    assert_eq!(b(0.0, 1.0, f64::NAN), bad);
    assert_eq!(b(-f64::MAX, f64::MAX, 0.0), bad);
    assert_eq!(b(f64::NEG_INFINITY, 1.0, 0.5), bad);
    assert_eq!(b(0.0, f64::INFINITY, 0.5), bad);
    assert!(b(0.0, 1.0, 0.5).is_none());

    let mut p = EProcess::new(Score::Sign, Tail::Upper, f64::INFINITY).unwrap();
    run(&mut p, &[1.0, 1.0]);
    for x in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(p.observe(x), Err(Error::NotFinite));
    }
    close(p.value(), 1.5);
    assert_eq!(p.observed(), 2);

    let s = Score::Bounded {
        lo: 0.0,
        hi: 1.0,
        mean: 0.5,
    };
    let mut q = EProcess::new(s, Tail::Upper, f64::INFINITY).unwrap();
    q.observe(1.0).unwrap();
    assert_eq!(q.observe(1.5), Err(Error::OutOfBounds));
    assert_eq!(q.observe(-0.1), Err(Error::OutOfBounds));
    assert_eq!(q.observed(), 1);
    q.observe(0.0).unwrap();
    q.observe(1.0).unwrap();
}

#[test]
fn a_stopped_process_still_refuses_bad_observations() {
    let s = Score::Bounded {
        lo: 0.0,
        hi: 1.0,
        mean: 0.5,
    };
    let mut p = EProcess::new(s, Tail::Upper, 1.1).unwrap();
    run(&mut p, &[1.0, 1.0]);
    assert!(p.stopped());
    assert_eq!(p.observe(1.5), Err(Error::OutOfBounds));
    assert_eq!(p.observe(-0.1), Err(Error::OutOfBounds));
    assert_eq!(p.observe(f64::NAN), Err(Error::NotFinite));
    assert_eq!(p.observe(1.0), Ok(()));
    assert_eq!(p.observed(), 2);
}

#[test]
fn error_messages_name_the_cause() {
    assert_eq!(
        Error::Parameter("alpha").to_string(),
        "invalid parameter: alpha"
    );
    assert_eq!(Error::NotFinite.to_string(), "observation is not finite");
    assert_eq!(
        Error::OutOfBounds.to_string(),
        "observation is outside the declared bounds"
    );
}

#[test]
fn long_runs_stay_finite_or_infinite_never_nan() {
    let mut p = EProcess::new(Score::Sign, Tail::Both, f64::INFINITY).unwrap();
    for _ in 0..20_000 {
        p.observe(1.0).unwrap();
    }
    assert!(!p.value().is_nan());
    assert!(p.value() >= 1e6);
    // An infinite stop never fires, even once the value reaches +inf.
    assert!(!p.stopped());
    assert_eq!(p.observed(), 20_000);
    assert!(p.p_value() >= 0.0 && p.p_value() <= 1e-6);

    let mut q = EProcess::new(Score::Sign, Tail::Both, f64::INFINITY).unwrap();
    for i in 0..20_000 {
        q.observe(if i % 2 == 0 { 1.0 } else { -1.0 }).unwrap();
    }
    assert!(q.value().is_finite() && q.value() > 0.0);
}

fn stopped_share<F: FnMut(&mut Rng) -> f64>(
    score: Score,
    tail: Tail,
    n_seq: usize,
    mut draw: F,
    seed: u8,
) -> f64 {
    let mut rng = Rng::from_seed([seed; 32]);
    let mut hits = 0;
    for _ in 0..n_seq {
        let mut p = EProcess::new(score, tail, 20.0).unwrap();
        for _ in 0..30 {
            p.observe(draw(&mut rng)).unwrap();
        }
        hits += usize::from(p.stopped());
    }
    hits as f64 / n_seq as f64
}

#[test]
fn type_one_error_holds_under_each_null() {
    let clip = stopped_share(
        Score::Clip { scale: 1.0 },
        Tail::Both,
        20_000,
        |r| r.normal(),
        1,
    );
    assert!(clip <= 0.05, "{clip}");
    assert!(clip > 0.0);
    let sign = stopped_share(Score::Sign, Tail::Upper, 20_000, |r| r.normal(), 2);
    assert!(sign <= 0.05, "{sign}");
    let s = Score::Bounded {
        lo: 0.0,
        hi: 1.0,
        mean: 0.5,
    };
    let bounded = stopped_share(s, Tail::Upper, 20_000, |r| r.uniform(), 3);
    assert!(bounded <= 0.05, "{bounded}");
}

#[test]
fn detects_a_one_sd_effect() {
    let share = stopped_share(
        Score::Clip { scale: 1.0 },
        Tail::Both,
        2_000,
        |r| 1.0 + r.normal(),
        4,
    );
    assert!(share >= 0.9, "{share}");
}

#[test]
fn a_stopped_process_never_reads_below_its_stop() {
    let mut p = EProcess::new(Score::Sign, Tail::Upper, 20.0).unwrap();
    let mut i = 0;
    while !p.stopped() {
        p.observe(1.0).unwrap();
        i += 1;
        assert!(i < 100);
    }
    assert!(p.value() >= 20.0);
}

const UNIT: Score = Score::Bounded {
    lo: -1.0,
    hi: 1.0,
    mean: 0.0,
};

/// Scores that rise, meet the far end twice, then rise again, so the caps part ways.
const MIXED: [f64; 12] = [
    -0.5, -0.5, -0.5, -0.5, -0.5, -0.5, 1.0, 1.0, -0.8, -0.8, 0.9, -0.3,
];

fn capped(tail: Tail, stop: f64, caps: &[f64]) -> EProcess {
    EProcess::with_caps(UNIT, tail, stop, caps).unwrap()
}

/// The value after each score of `MIXED` and the peak, as the e-process gave them when its one
/// cap was a constant (commit b7f970b). An Upper test reads the scores reversed and gives the
/// values of the Lower one. The tolerance is four units in the last place, for a platform
/// whose `exp` or `ln_1p` rounds another way.
#[test]
fn new_gives_the_values_of_the_one_built_in_cap() {
    const ONE_SIDE: ([u64; 12], u64) = (
        [
            0x3ff0000000000000,
            0x3ff3333333333333,
            0x3ff9999999999999,
            0x400199999999999a,
            0x4008333333333332,
            0x4010a33333333332,
            0x3ff0a33333333332,
            0x3fdc857c57c57c56,
            0x3fe0cbc389055d22,
            0x3fe5805ded2ab9f7,
            0x3fd997e92879b909,
            0x3fdb92f68a64d7ac,
        ],
        0x4010a33333333332,
    );
    const BOTH: ([u64; 12], u64) = (
        [
            0x3ff0000000000000,
            0x3ff199999999999a,
            0x3ff4cccccccccccd,
            0x3ff999999999999a,
            0x4000199999999999,
            0x4004a33333333332,
            0x3ff0519999999999,
            0x3fe7215f15f15f16,
            0x3fe865e1c482ae91,
            0x3feac02ef6955cfc,
            0x3fe665fa4a1e6e42,
            0x3fe6e4bda29935eb,
        ],
        0x4004a33333333332,
    );
    let same = |got: f64, bits: u64| {
        let want = f64::from_bits(bits);
        assert!(
            (got - want).abs() <= 4.0 * f64::EPSILON * want,
            "{got} != {want}"
        );
    };
    for (tail, sign, (values, peak)) in [
        (Tail::Lower, 1.0, ONE_SIDE),
        (Tail::Upper, -1.0, ONE_SIDE),
        (Tail::Both, 1.0, BOTH),
    ] {
        let mut p = EProcess::new(UNIT, tail, f64::INFINITY).unwrap();
        for (got, bits) in run(&mut p, &MIXED.map(|x| sign * x))
            .into_iter()
            .zip(values)
        {
            same(got, bits);
        }
        same(p.peak(), peak);
    }
}

#[test]
fn the_cap_bounds_every_bet() {
    // Scores of -1/2 push the Lower bet past 1 from the fourth on, so it bets the cap: a
    // further -1/2 multiplies wealth by 1 + cap / 2, and a score at the far end by 1 - cap.
    for cap in [0.5, 0.9] {
        let mut p = capped(Tail::Lower, f64::INFINITY, &[cap]);
        let v = run(&mut p, &[vec![-0.5; 10], vec![1.0]].concat());
        close(v[9] / v[8], 1.0 + cap / 2.0);
        close(v[10] / v[9], 1.0 - cap);
    }
}

#[test]
fn a_bad_list_of_caps_is_refused() {
    let refused = |caps: &[f64]| {
        let p = EProcess::with_caps(UNIT, Tail::Lower, f64::INFINITY, caps);
        assert_eq!(p.unwrap_err(), Error::Parameter("cap"), "{caps:?}");
    };
    refused(&[]);
    for cap in [0.0, 1.0, -0.1, 1.5, f64::NAN, f64::INFINITY] {
        refused(&[cap]);
        refused(&[0.5, cap]);
    }
}

#[test]
fn several_caps_give_the_mean_of_their_e_values() {
    let caps = [0.5, 0.75, 0.9];
    for tail in [Tail::Lower, Tail::Upper, Tail::Both] {
        // An Upper test bets on positive scores, so it reads the scores reversed.
        let sign = if tail == Tail::Upper { -1.0 } else { 1.0 };
        let xs = MIXED.map(|x| sign * x);
        let each: Vec<Vec<f64>> = (caps.iter())
            .map(|&c| run(&mut capped(tail, f64::INFINITY, &[c]), &xs))
            .collect();
        let mixed = run(&mut capped(tail, f64::INFINITY, &caps), &xs);
        for (i, &v) in mixed.iter().enumerate() {
            close(v, (each[0][i] + each[1][i] + each[2][i]) / 3.0);
        }
        // The caps do part ways on these scores.
        assert!((each[0][11] - each[2][11]).abs() > 0.05, "{each:?}");
    }
}

/// After six rising scores, a score at the far end costs the cap of 0.9 nine tenths of its
/// wealth and the cap of 1/2 one half. Four more rising scores carry the lower cap past its
/// old peak and leave the higher one below its own, so the two peak at different scores.
#[test]
fn the_peak_is_the_largest_mean_not_the_mean_of_the_peaks() {
    let xs = [vec![-0.5; 6], vec![1.0], vec![-0.5; 4]].concat();
    let caps = [0.5, 0.9];
    let peaks: Vec<f64> = (caps.iter())
        .map(|&c| {
            let mut p = capped(Tail::Lower, f64::INFINITY, &[c]);
            run(&mut p, &xs);
            p.peak()
        })
        .collect();
    let mut p = capped(Tail::Lower, f64::INFINITY, &caps);
    let largest = run(&mut p, &xs).into_iter().fold(1.0, f64::max);
    close(p.peak(), largest);
    close(p.p_value(), 1.0 / largest);
    assert!(
        largest < (peaks[0] + peaks[1]) / 2.0 - 0.1,
        "{largest} {peaks:?}"
    );
}

#[test]
fn stop_freezes_the_mean_over_caps() {
    let caps = [0.5, 0.9];
    let values = run(&mut capped(Tail::Lower, f64::INFINITY, &caps), &[-0.9; 12]);
    let first = values.iter().position(|&v| v >= 20.0).unwrap();
    let mut p = capped(Tail::Lower, 20.0, &caps);
    let stopped = run(&mut p, &[-0.9; 12]);
    assert!(p.stopped());
    assert_eq!(stopped[first], values[first]);
    assert_eq!(stopped[11], values[first]);
    assert!(first < 11 && values[first - 1] < 20.0);
}
