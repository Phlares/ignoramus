use evidence::rng::Rng;

fn seq_seed() -> [u8; 32] {
    let mut s = [0u8; 32];
    for (i, b) in s.iter_mut().enumerate() {
        *b = i as u8;
    }
    s
}

#[test]
fn matches_reference_vectors() {
    let mut s = [0u8; 32];
    for (i, w) in [1u8, 2, 3, 4].iter().enumerate() {
        s[i * 8] = *w;
    }
    let mut r = Rng::from_seed(s);
    let got: Vec<u64> = (0..6).map(|_| r.next_u64()).collect();
    assert_eq!(
        got,
        [
            11520,
            0,
            1509978240,
            1215971899390074240,
            1216172134540287360,
            607988272756665600
        ]
    );
    let mut r = Rng::from_seed(seq_seed());
    let got: Vec<u64> = (0..3).map(|_| r.next_u64()).collect();
    assert_eq!(
        got,
        [13557399450712487245, 2706373525000986293, 88962455226424365]
    );
}

#[test]
fn zero_seed_uses_the_fallback_state() {
    let mut r = Rng::from_seed([0; 32]);
    let got: Vec<u64> = (0..4).map(|_| r.next_u64()).collect();
    assert_eq!(
        got,
        [
            0,
            16155200969329072355,
            16155200969329072355,
            11268428918147242265
        ]
    );
}

#[test]
fn uniform_maps_the_top_53_bits_to_zero_exclusive_one_inclusive() {
    let mut a = Rng::from_seed(seq_seed());
    let mut b = Rng::from_seed(seq_seed());
    for _ in 0..1000 {
        let want = ((b.next_u64() >> 11) + 1) as f64 / 9007199254740992.0;
        assert_eq!(a.uniform(), want);
    }
    // Reference seed 1,2,3,4: the second raw draw is 0, so uniform must be 2^-53.
    let mut s = [0u8; 32];
    for (i, w) in [1u8, 2, 3, 4].iter().enumerate() {
        s[i * 8] = *w;
    }
    let mut r = Rng::from_seed(s);
    r.next_u64();
    assert_eq!(r.uniform(), 1.0 / 9007199254740992.0);
}

#[test]
fn normal_is_box_muller_of_two_uniforms() {
    let mut a = Rng::from_seed(seq_seed());
    let mut b = Rng::from_seed(seq_seed());
    for _ in 0..10 {
        let (u1, u2) = (b.uniform(), b.uniform());
        let want = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
        assert!((a.normal() - want).abs() < 1e-12);
    }
}

#[test]
fn uniform_is_in_half_open_unit_interval() {
    let mut r = Rng::from_seed(seq_seed());
    let n = 100_000;
    let mut sum = 0.0;
    for _ in 0..n {
        let u = r.uniform();
        assert!(0.0 < u && u <= 1.0);
        sum += u;
    }
    assert!((sum / n as f64 - 0.5).abs() < 0.005);
}

#[test]
fn normal_has_unit_moments() {
    let mut r = Rng::from_seed(seq_seed());
    let n = 100_000;
    let xs: Vec<f64> = (0..n).map(|_| r.normal()).collect();
    let mean = xs.iter().sum::<f64>() / n as f64;
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
    let tail = xs.iter().filter(|x| **x > 1.959964).count() as f64 / n as f64;
    assert!(mean.abs() < 0.01, "mean {mean}");
    assert!((0.98..=1.02).contains(&var), "var {var}");
    assert!((0.022..=0.028).contains(&tail), "tail {tail}");
}

/// `below` by its rule, in 128-bit arithmetic: draw x, draw again while
/// x ≥ 2^64 − (2^64 mod n), return x mod n; n = 0 stands for 2^64. Counts the raw draws.
fn below_by_rule(r: &mut Rng, n: u64, raw: &mut u32) -> u64 {
    let span = 1u128 << 64;
    let n = if n == 0 { span } else { u128::from(n) };
    let limit = span - span % n;
    loop {
        *raw += 1;
        let x = u128::from(r.next_u64());
        if x < limit {
            return (x % n) as u64;
        }
    }
}

#[test]
fn below_rejects_the_draws_past_the_last_whole_multiple_of_n() {
    for n in [1, 2, 3, 7, 1 << 32, (1 << 63) + 1, u64::MAX, 0] {
        let (mut a, mut b) = (Rng::from_seed(seq_seed()), Rng::from_seed(seq_seed()));
        let mut raw = 0;
        for _ in 0..400 {
            let got = a.below(n);
            assert_eq!(got, below_by_rule(&mut b, n, &mut raw), "n = {n}");
            assert!(n == 0 || got < n, "n = {n}");
        }
        // Both read the same raw draws, rejected ones included.
        assert_eq!(a.next_u64(), b.next_u64(), "n = {n}");
        // 2^63 + 1 rejects about half the draws; a power of two rejects none.
        match n {
            0x8000_0000_0000_0001 => assert!(raw > 600, "{raw} raw draws"),
            0..=2 => assert_eq!(raw, 400),
            _ => {}
        }
    }
}

/// The first `k` positions of a forward Fisher-Yates shuffle of a full array `0..available`:
/// step i swaps position i with i + below(available − i).
fn dense_sample(r: &mut Rng, available: u64, k: u64) -> Vec<u64> {
    let mut a: Vec<u64> = (0..available).collect();
    let k = k.min(available);
    for i in 0..k {
        let j = i + r.below(available - i);
        a.swap(i as usize, j as usize);
    }
    a.truncate(k as usize);
    a
}

#[test]
fn sample_equals_the_dense_forward_fisher_yates() {
    let cases = [
        (0, 0),
        (0, 3),
        (1, 1),
        (2, 2),
        (5, 5),
        (10, 3),
        (10, 10),
        (10, 25),
        (100, 40),
        (1000, 999),
    ];
    for (available, k) in cases {
        let (mut a, mut b) = (Rng::from_seed(seq_seed()), Rng::from_seed(seq_seed()));
        let got = a.sample(available, k);
        assert_eq!(got, dense_sample(&mut b, available, k), "{available} {k}");
        assert_eq!(got.len() as u64, k.min(available));
        assert_eq!(a.next_u64(), b.next_u64(), "{available} {k}");
    }
}

#[test]
fn sample_takes_distinct_values_from_a_range_too_large_to_list() {
    let mut r = Rng::from_seed(seq_seed());
    let got = r.sample(u64::MAX, 2000);
    assert_eq!(got.len(), 2000);
    let distinct: std::collections::BTreeSet<_> = got.iter().collect();
    assert_eq!(distinct.len(), 2000);
    assert!(got.iter().all(|x| *x < u64::MAX));
}
