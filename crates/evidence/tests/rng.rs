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
fn zero_seed_still_draws() {
    let mut r = Rng::from_seed([0; 32]);
    let v: Vec<u64> = (0..4).map(|_| r.next_u64()).collect();
    assert!(v.iter().any(|x| *x != v[0]));
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
