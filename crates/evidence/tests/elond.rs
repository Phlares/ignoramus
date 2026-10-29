use evidence::Error;
use evidence::elond::{Gamma, Stream};

fn close_to(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} vs {b}");
}

#[test]
fn bounded_gamma_spreads_evenly() {
    let g = Gamma::Bounded { m: 4 };
    for t in 1..=4 {
        assert_eq!(g.at(t), 0.25);
    }
    assert_eq!(g.at(5), 0.0);
    assert_eq!(g.at(0), 0.0);
}

#[test]
fn unbounded_gamma_matches_published_sequence() {
    let g = Gamma::Unbounded;
    close_to(g.at(1), 0.05351677091260086, 1e-15);
    close_to(g.at(2), 0.011638205782941741, 1e-15);
    close_to(g.at(10), 0.0038982519052638913, 1e-15);
    assert_eq!(g.at(0), 0.0);
    let sum: f64 = (1..=1_000_000).map(|t| g.at(t)).sum();
    assert!(sum > 0.52 && sum < 0.53 && sum < 1.0, "{sum}");
}

#[test]
fn levels_grow_with_closed_discoveries() {
    let mut s = Stream::new(0.0625, Gamma::Bounded { m: 4 }).unwrap();
    let a = s.open();
    assert_eq!(a.index(), 1);
    assert_eq!(a.level(), 1.0 / 64.0);
    assert_eq!(s.close(a, 64.0), Ok(true));
    let b = s.open();
    assert_eq!(b.index(), 2);
    assert_eq!(b.level(), 1.0 / 32.0);
    assert_eq!(s.close(b, 31.9), Ok(false));
    let c = s.open();
    assert_eq!(c.level(), 1.0 / 32.0);
    assert_eq!(s.discoveries(), 1);
    assert_eq!(s.opened(), 3);

    let mut fresh = Stream::new(0.0625, Gamma::Bounded { m: 4 }).unwrap();
    let a = fresh.open();
    assert_eq!(fresh.close(a, 63.9), Ok(false));
    assert_eq!(fresh.discoveries(), 0);
}

#[test]
fn open_slots_use_discoveries_closed_so_far() {
    let mut s = Stream::new(0.0625, Gamma::Bounded { m: 4 }).unwrap();
    let a = s.open();
    let b = s.open();
    assert_eq!(s.close(a, 1e9), Ok(true));
    let c = s.open();
    assert_eq!(b.level(), 1.0 / 64.0);
    assert_eq!(c.level(), 1.0 / 32.0);
}

#[test]
fn an_abandoned_slot_keeps_its_index() {
    let mut s = Stream::new(0.0625, Gamma::Bounded { m: 4 }).unwrap();
    let _a = s.open();
    let b = s.open();
    assert_eq!(b.index(), 2);
    assert_eq!(b.level(), 1.0 / 64.0);
    assert_eq!(s.opened(), 2);
    assert_eq!(s.discoveries(), 0);
}

#[test]
fn past_the_bound_nothing_is_discovered() {
    let mut s = Stream::new(0.5, Gamma::Bounded { m: 1 }).unwrap();
    let a = s.open();
    assert_eq!(s.close(a, 0.0), Ok(false));
    let b = s.open();
    assert_eq!(b.index(), 2);
    assert_eq!(b.level(), 0.0);
    assert_eq!(s.close(b, f64::INFINITY), Ok(false));
    assert_eq!(s.discoveries(), 0);
}

#[test]
fn infinite_evidence_is_a_discovery() {
    let mut s = Stream::new(0.05, Gamma::Unbounded).unwrap();
    let a = s.open();
    assert_eq!(s.close(a, f64::INFINITY), Ok(true));
    assert_eq!(s.discoveries(), 1);
}

#[test]
fn rejects_bad_parameters() {
    let g = Gamma::Unbounded;
    for alpha in [0.0, 1.0, f64::NAN, -0.1, 1.5] {
        assert_eq!(Stream::new(alpha, g).err(), Some(Error::Parameter("alpha")));
    }
    assert_eq!(
        Stream::new(0.05, Gamma::Bounded { m: 0 }).err(),
        Some(Error::Parameter("m"))
    );
    let mut s = Stream::new(0.05, g).unwrap();
    for e in [f64::NAN, -1.0, f64::NEG_INFINITY] {
        let slot = s.open();
        assert_eq!(s.close(slot, e), Err(Error::Parameter("e-value")));
        assert_eq!(s.discoveries(), 0);
    }
    assert_eq!(s.opened(), 3);
}

#[test]
fn level_is_alpha_times_gamma_times_discoveries_plus_one() {
    let mut s = Stream::new(0.05, Gamma::Unbounded).unwrap();
    let a = s.open();
    assert_eq!(a.level(), 0.05 * Gamma::Unbounded.at(1));
    s.close(a, f64::INFINITY).unwrap();
    let b = s.open();
    assert_eq!(b.level(), 0.05 * Gamma::Unbounded.at(2) * 2.0);
}
