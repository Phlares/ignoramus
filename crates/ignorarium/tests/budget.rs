use ignorarium::budget::{
    ALERT_FRACTIONS, Alerts, Budget, Cost, CostMeter, Decision, FakeMeter, HostMeter, MeterError,
    PauseReason, SAFETY_MARGIN, crossed, decide,
};
use std::time::{Duration, Instant};

const CAP: Budget = Budget {
    cpu_hours: 1.0,
    wall_hours: 1.0,
    money_usd: 0.0,
};

fn wall(frac: f64) -> Cost {
    Cost {
        wall_seconds: 3600.0 * frac,
        ..Cost::default()
    }
}

fn cpu(frac: f64) -> Cost {
    Cost {
        cpu_seconds: 3600.0 * frac,
        ..Cost::default()
    }
}

fn usd(u: f64) -> Cost {
    Cost {
        usd: u,
        ..Cost::default()
    }
}

fn ok() -> Result<(), MeterError> {
    Ok(())
}

#[test]
fn constants_hold_the_agreed_values() {
    assert_eq!(ALERT_FRACTIONS, [0.50, 0.80, 0.95]);
    assert_eq!(SAFETY_MARGIN, 1.0);
}

#[test]
fn crossed_lists_each_fraction_between_prev_and_now() {
    let none = Cost::default();
    assert_eq!(crossed(&none, &wall(0.49), &CAP), vec![]);
    // A fraction counts at equality, once.
    assert_eq!(crossed(&none, &wall(0.5), &CAP), vec![('w', 0.5)]);
    assert_eq!(crossed(&wall(0.5), &wall(0.79), &CAP), vec![]);
    assert_eq!(
        crossed(&wall(0.4), &wall(0.97), &CAP),
        vec![('w', 0.5), ('w', 0.8), ('w', 0.95)]
    );
    assert_eq!(crossed(&wall(0.94), &wall(0.95), &CAP), vec![('w', 0.95)]);
    // Past the cap still reports nothing new.
    assert_eq!(crossed(&wall(0.95), &wall(2.0), &CAP), vec![]);
}

#[test]
fn crossed_names_each_dimension() {
    let b = Budget {
        cpu_hours: 1.0,
        wall_hours: 2.0,
        money_usd: 4.0,
    };
    let now = Cost {
        cpu_seconds: 1800.0,
        wall_seconds: 3600.0,
        usd: 2.0,
    };
    assert_eq!(
        crossed(&Cost::default(), &now, &b),
        vec![('c', 0.5), ('w', 0.5), ('m', 0.5)]
    );
}

#[test]
fn alerts_fire_once_each_at_50_80_95() {
    let meter = FakeMeter::new(vec![
        Ok(wall(0.4)),
        Ok(wall(0.6)),
        Ok(wall(0.85)),
        Ok(wall(0.97)),
        Ok(wall(0.97)),
    ]);
    let mut alerts = Alerts::seeded(&meter.sample().unwrap(), &CAP);
    let mut fired = Vec::new();
    for _ in 0..4 {
        fired.extend(alerts.fresh(&meter.sample().unwrap(), &CAP));
    }
    assert_eq!(fired, vec![('w', 0.5), ('w', 0.8), ('w', 0.95)]);
}

#[test]
fn a_dip_and_rise_does_not_repeat_an_alert() {
    let mut alerts = Alerts::seeded(&Cost::default(), &CAP);
    assert_eq!(alerts.fresh(&wall(0.6), &CAP), vec![('w', 0.5)]);
    assert_eq!(alerts.fresh(&wall(0.3), &CAP), vec![]);
    assert_eq!(alerts.fresh(&wall(0.6), &CAP), vec![]);
}

#[test]
fn alerts_seeded_with_earlier_spend_skip_what_is_already_crossed() {
    let mut alerts = Alerts::seeded(&wall(0.85), &CAP);
    assert_eq!(alerts.fresh(&wall(0.85), &CAP), vec![]);
    assert_eq!(alerts.fresh(&wall(0.96), &CAP), vec![('w', 0.95)]);
}

#[test]
fn planted_overspend_pauses_before_the_crossing_row() {
    let next = cpu(0.1);
    assert_eq!(decide(&cpu(0.89), &next, &CAP, ok()), Decision::Continue);
    // 0.9 + 0.1 reaches the cap: the row must not start.
    assert_eq!(
        decide(&cpu(0.9), &next, &CAP, ok()),
        Decision::Pause(PauseReason::WouldCross('c'))
    );
    assert_eq!(
        decide(&cpu(0.95), &next, &CAP, ok()),
        Decision::Pause(PauseReason::WouldCross('c'))
    );
}

#[test]
fn cap_reached_pauses_at_the_boundary() {
    let zero = Cost::default();
    assert_eq!(decide(&wall(0.999), &zero, &CAP, ok()), Decision::Continue);
    assert_eq!(
        decide(&wall(1.0), &zero, &CAP, ok()),
        Decision::Pause(PauseReason::CapReached('w'))
    );
    assert_eq!(
        decide(&wall(1.5), &cpu(0.1), &CAP, ok()),
        Decision::Pause(PauseReason::CapReached('w'))
    );
}

#[test]
fn money_binds_when_it_has_a_cap() {
    let b = Budget {
        cpu_hours: 0.0,
        wall_hours: 0.0,
        money_usd: 2.0,
    };
    assert_eq!(decide(&usd(1.0), &usd(0.5), &b, ok()), Decision::Continue);
    assert_eq!(
        decide(&usd(1.5), &usd(0.5), &b, ok()),
        Decision::Pause(PauseReason::WouldCross('m'))
    );
    assert_eq!(
        decide(&usd(2.0), &usd(0.0), &b, ok()),
        Decision::Pause(PauseReason::CapReached('m'))
    );
}

#[test]
fn meter_fault_pauses_the_run() {
    let meter = FakeMeter::new(vec![Err(MeterError("no reading".into()))]);
    let e = meter.sample().unwrap_err();
    assert_eq!(
        decide(&Cost::default(), &Cost::default(), &CAP, Err(e)),
        Decision::Pause(PauseReason::MeterFault("no reading".into()))
    );
}

#[test]
fn a_zero_dimension_has_no_cap_but_others_bind() {
    // money_usd = 0 in CAP: any usd is fine; cpu still binds.
    let lots = usd(1e9);
    assert_eq!(decide(&lots, &lots, &CAP, ok()), Decision::Continue);
    assert_eq!(crossed(&Cost::default(), &lots, &CAP), vec![]);
    assert_eq!(
        decide(&cpu(1.0), &lots, &CAP, ok()),
        Decision::Pause(PauseReason::CapReached('c'))
    );
}

fn internal(d: Decision) -> bool {
    matches!(d, Decision::Pause(PauseReason::InternalCheck(_)))
}

#[test]
fn a_reading_that_is_not_a_number_pauses() {
    let zero = Cost::default();
    let nan = Cost {
        wall_seconds: f64::NAN,
        ..zero
    };
    let inf = Cost {
        cpu_seconds: f64::INFINITY,
        ..zero
    };
    assert!(internal(decide(&nan, &zero, &CAP, ok())));
    assert!(internal(decide(&inf, &zero, &CAP, ok())));
    assert!(internal(decide(&usd(-1.0), &zero, &CAP, ok())));
    assert!(internal(decide(&zero, &nan, &CAP, ok())));
    let cap_bad = Budget {
        wall_hours: f64::NAN,
        ..CAP
    };
    assert!(internal(decide(&zero, &zero, &cap_bad, ok())));
}

#[test]
fn pause_reasons_read_plainly() {
    let s = |r: PauseReason| r.to_string();
    assert_eq!(s(PauseReason::CapReached('w')), "the wall cap is reached");
    assert_eq!(
        s(PauseReason::WouldCross('c')),
        "the next row would reach the cpu cap"
    );
    assert_eq!(
        s(PauseReason::WouldCross('m')),
        "the next row would reach the money cap"
    );
    assert_eq!(
        s(PauseReason::MeterFault("x".into())),
        "the cost meter failed: x"
    );
    assert_eq!(
        s(PauseReason::InternalCheck("y".into())),
        "an internal check failed: y"
    );
}

#[test]
fn larger_takes_each_dimension_separately() {
    let a = Cost {
        cpu_seconds: 1.0,
        wall_seconds: 5.0,
        usd: 3.0,
    };
    let b = Cost {
        cpu_seconds: 2.0,
        wall_seconds: 4.0,
        usd: 3.0,
    };
    let want = Cost {
        cpu_seconds: 2.0,
        wall_seconds: 5.0,
        usd: 3.0,
    };
    assert_eq!(a.larger(&b), want);
}

#[test]
fn host_meter_adds_earlier_spend_rows_and_elapsed_time() {
    let prior = Cost {
        cpu_seconds: 100.0,
        wall_seconds: 1800.0,
        usd: 99.0,
    };
    let mut m = HostMeter::new(prior, 2.0);
    m.start = Instant::now()
        .checked_sub(Duration::from_secs(1800))
        .unwrap();
    m.add_row(50.0);
    m.add_row(25.0);
    let c = m.sample().unwrap();
    assert_eq!(c.cpu_seconds, 175.0);
    assert!(
        (3600.0..3700.0).contains(&c.wall_seconds),
        "{}",
        c.wall_seconds
    );
    // Money comes from total wall time, not from the prior usd.
    assert_eq!(c.usd, c.wall_seconds / 3600.0 * 2.0);
}

#[test]
fn fake_meter_repeats_its_last_reading_and_faults_when_empty() {
    let m = FakeMeter::new(vec![Ok(wall(0.1)), Ok(wall(0.2))]);
    assert_eq!(m.sample().unwrap(), wall(0.1));
    assert_eq!(m.sample().unwrap(), wall(0.2));
    assert_eq!(m.sample().unwrap(), wall(0.2));
    assert!(FakeMeter::new(vec![]).sample().is_err());
}
