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
