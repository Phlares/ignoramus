use evidence::rng::Rng;
use evidence::scan::{Scan, from_null, sign_flip};
use evidence::{Error, Tail};

fn rng(tag: u8) -> Rng {
    let mut s = [0u8; 32];
    s[0] = tag;
    s[31] = 7;
    Rng::from_seed(s)
}

#[test]
fn from_null_counts_ties_as_extreme() {
    let null = vec![
        vec![1.0, 0.0],
        vec![4.0, 2.0],
        vec![2.0, 2.0],
        vec![0.0, 0.5],
    ];
    let got = from_null(&[3.0, 1.0], &null).unwrap();
    assert_eq!(
        got,
        Scan {
            local: vec![0.4, 0.6],
            global: vec![0.4, 0.8]
        }
    );
}

#[test]
fn empty_null_gives_one() {
    let got = from_null(&[5.0], &[]).unwrap();
    assert_eq!(got.local, [1.0]);
    assert_eq!(got.global, [1.0]);
}

#[test]
fn from_null_rejects_bad_inputs() {
    let ok = vec![vec![1.0, 2.0]];
    assert_eq!(
        from_null(&[1.0, 2.0], &[vec![1.0]]),
        Err(Error::Parameter("null"))
    );
    assert_eq!(
        from_null(&[1.0, 2.0], &[vec![1.0, 2.0, 3.0]]),
        Err(Error::Parameter("null"))
    );
    assert_eq!(from_null(&[f64::NAN, 2.0], &ok), Err(Error::NotFinite));
    assert_eq!(from_null(&[1.0, f64::INFINITY], &ok), Err(Error::NotFinite));
    assert_eq!(
        from_null(&[1.0, 2.0], &[vec![1.0, f64::NAN]]),
        Err(Error::NotFinite)
    );
    assert_eq!(
        from_null(&[1.0, 2.0], &[vec![f64::NEG_INFINITY, 2.0]]),
        Err(Error::NotFinite)
    );
}

#[test]
fn flips_are_shared_across_conditions() {
    let c: Vec<f64> = (0..70).map(|i| ((i * 37 % 11) as f64) - 4.5).collect();
    let got = sign_flip(&[c.clone(), c], Tail::Both, 300, &mut rng(1)).unwrap();
    assert_eq!(got.local, got.global);
    assert_eq!(got.local[0], got.local[1]);
}

#[test]
fn a_strong_condition_has_a_small_local_p() {
    let d = vec![vec![5.0; 10]];
    let up = sign_flip(&d, Tail::Upper, 2000, &mut rng(2)).unwrap();
    assert!(up.local[0] < 0.01, "{:?}", up);
    let lo = sign_flip(&d, Tail::Lower, 2000, &mut rng(2)).unwrap();
    assert_eq!(lo.local[0], 1.0);
    let both = sign_flip(&d, Tail::Both, 2000, &mut rng(2)).unwrap();
    assert!(both.local[0] < 0.01, "{:?}", both);
}

#[test]
fn zero_draws_gives_one() {
    let got = sign_flip(&[vec![1.0, 2.0]], Tail::Upper, 0, &mut rng(3)).unwrap();
    assert_eq!(got.local, [1.0]);
    assert_eq!(got.global, [1.0]);
}

#[test]
fn all_zero_condition_is_one_not_nan() {
    let d = vec![vec![0.0; 3], vec![1.0, 2.0, 3.0]];
    let got = sign_flip(&d, Tail::Upper, 100, &mut rng(4)).unwrap();
    assert_eq!(got.local[0], 1.0);
    assert!(got.local.iter().chain(&got.global).all(|p| !p.is_nan()));
}

#[test]
fn global_is_never_below_local() {
    let mut r = rng(5);
    let d: Vec<Vec<f64>> = (0..5)
        .map(|k| (0..20).map(|_| r.normal() + 0.2 * k as f64).collect())
        .collect();
    let got = sign_flip(&d, Tail::Both, 500, &mut r).unwrap();
    for k in 0..5 {
        assert!(got.global[k] >= got.local[k]);
    }
    assert!(got.global.iter().any(|g| *g > 0.0 && *g < 1.0));
}

#[test]
fn signs_come_from_the_rng_bits_lsb_first_with_64_units_per_word() {
    // Bit set = +1, so the null statistic equals the observed one on those draws.
    let draws = 400;
    let mut probe = rng(6);
    let plus = (0..draws).filter(|_| probe.next_u64() & 1 == 1).count();
    let got = sign_flip(&[vec![2.0]], Tail::Upper, draws, &mut rng(6)).unwrap();
    assert_eq!(got.local[0], (1 + plus) as f64 / (draws + 1) as f64);
    // 65 units take two words per draw; unit 64 is bit 0 of the second word.
    let mut probe = rng(6);
    let mut plus = 0;
    let mut d = vec![0.0; 65];
    d[64] = 3.0;
    for _ in 0..draws {
        probe.next_u64();
        if probe.next_u64() & 1 == 1 {
            plus += 1;
        }
    }
    let got = sign_flip(&[d], Tail::Upper, draws, &mut rng(6)).unwrap();
    assert_eq!(got.local[0], (1 + plus) as f64 / (draws + 1) as f64);
}

#[test]
fn scan_holds_its_level_under_the_null() {
    let mut r = rng(8);
    let (k, n, sets) = (5, 20, 1000);
    let mut hits = 0;
    for _ in 0..sets {
        let shared: Vec<f64> = (0..n).map(|_| r.normal()).collect();
        let d: Vec<Vec<f64>> = (0..k)
            .map(|_| {
                shared
                    .iter()
                    .map(|z| 0.5f64.sqrt() * z + 0.5f64.sqrt() * r.normal())
                    .collect()
            })
            .collect();
        let s = sign_flip(&d, Tail::Both, 200, &mut r).unwrap();
        if s.global.iter().cloned().fold(f64::INFINITY, f64::min) <= 0.05 {
            hits += 1;
        }
    }
    let rate = hits as f64 / sets as f64;
    assert!((0.02..=0.075).contains(&rate), "rate {rate}");
}

#[test]
fn rejects_bad_inputs() {
    let t = Tail::Both;
    let mut r = rng(9);
    let bad = Err(Error::Parameter("diffs"));
    assert_eq!(sign_flip(&[], t, 10, &mut r), bad);
    assert_eq!(sign_flip(&[vec![]], t, 10, &mut r), bad);
    assert_eq!(sign_flip(&[vec![1.0, 2.0], vec![1.0]], t, 10, &mut r), bad);
    assert_eq!(sign_flip(&[vec![1.0], vec![1.0, 2.0]], t, 10, &mut r), bad);
    let nf = Err(Error::NotFinite);
    assert_eq!(sign_flip(&[vec![1.0, f64::NAN]], t, 10, &mut r), nf);
    assert_eq!(sign_flip(&[vec![f64::INFINITY]], t, 10, &mut r), nf);
    assert_eq!(sign_flip(&[vec![1e200, 1e200]], t, 10, &mut r), nf);
}
