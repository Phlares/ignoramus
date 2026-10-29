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
    for (a, b) in v.iter().zip([1.0, 1.5, 0.75, 0.9375]) {
        close(*a, b);
    }
    close(p.p_value(), 1.0 / 1.5);
    assert_eq!(p.observed(), 4);
    assert_eq!(MAX_BET, 0.5);
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
    for (a, b) in v.iter().zip([1.0, 1.4, 1.05]) {
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
fn bounded_uses_the_longer_side_when_mean_is_high() {
    let s = Score::Bounded {
        lo: 0.0,
        hi: 1.0,
        mean: 0.75,
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
fn long_runs_stay_finite_or_infinite_never_nan() {
    let mut p = EProcess::new(Score::Sign, Tail::Both, f64::INFINITY).unwrap();
    for _ in 0..20_000 {
        p.observe(1.0).unwrap();
    }
    assert!(!p.value().is_nan());
    assert!(p.value() >= 1e6);
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
