use evidence::Error;
use evidence::elond::Gamma;
use evidence::rng::Rng;
use evidence::sim::{Estimate, Report, Scenario, simulate};

const SINGLE: u8 = 0;
const STREAM: u8 = 1;
const OTHER: u8 = 2;
/// One null child tested alone at level 0.05 over 20 units rejects about 1 time in
/// 1,300, so the family-wise error passes 0.05 only near 70 such children.
const DEFECT: usize = 100;

fn close_to(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} vs {b}");
}

/// Byte 0 holds the scenario index, byte 1 the test family.
fn seed(family: u8, index: usize) -> Rng {
    let mut s = [0u8; 32];
    s[0] = index as u8;
    s[1] = family;
    Rng::from_seed(s)
}

fn base() -> Scenario {
    Scenario {
        alpha: 0.05,
        gamma: Gamma::Bounded { m: 1 },
        trees: 1,
        children: 3,
        depth: 1,
        units: 20,
        rho: 0.0,
        theta: 2.0,
        non_null: 0.0,
        effect: 0.0,
        root_only: true,
        undeclared: 0,
    }
}

fn single_tree_grid() -> Vec<Scenario> {
    let mut grid = Vec::new();
    for rho in [0.0, 0.9] {
        for depth in [1, 2] {
            for theta in [2.0, 20.0] {
                for (non_null, effect) in [(0.0, 0.0), (1.0, 1.0)] {
                    grid.push(Scenario {
                        rho,
                        depth,
                        theta,
                        non_null,
                        effect,
                        ..base()
                    });
                }
            }
        }
    }
    grid
}

fn stream_grid() -> Vec<Scenario> {
    let mut grid = Vec::new();
    for gamma in [Gamma::Unbounded, Gamma::Bounded { m: 40 }] {
        for non_null in [0.0, 0.25] {
            grid.push(Scenario {
                gamma,
                trees: 40,
                units: 30,
                rho: 0.5,
                theta: 5.0,
                non_null,
                effect: 1.5,
                root_only: false,
                ..base()
            });
        }
    }
    grid
}

fn run(grid: &[Scenario], reps: usize, family: u8) -> Vec<Report> {
    grid.iter()
        .enumerate()
        .map(|(i, s)| {
            let r = simulate(s, reps, &mut seed(family, i)).unwrap();
            eprintln!(
                "{s:?} reps {reps}: fwer {:.5} (upper {:.5}) fdr {:.5} (upper {:.5}) power {:.5}",
                r.fwer.mean, r.fwer.upper, r.fdr.mean, r.fdr.upper, r.power.mean
            );
            r
        })
        .collect()
}

fn single_trees_hold(reps: usize) {
    let grid = single_tree_grid();
    for (s, r) in grid.iter().zip(run(&grid, reps, SINGLE)) {
        assert!(r.fwer.holds(0.05), "{s:?} {r:?}");
        assert!(r.fdr.holds(0.05), "{s:?} {r:?}");
    }
}

fn streams_hold(reps: usize) {
    let grid = stream_grid();
    for (s, r) in grid.iter().zip(run(&grid, reps, STREAM)) {
        assert!(r.fdr.holds(0.05), "{s:?} {r:?}");
        assert_eq!(r.power.mean > 0.0, s.non_null > 0.0, "{s:?} {r:?}");
    }
}

#[test]
fn wilson_matches_hand_values() {
    close_to(Estimate::of(0.0, 100).upper, 0.03699349820698568, 1e-12);
    close_to(Estimate::of(50.0, 100).upper, 0.5961684696340044, 1e-12);
    close_to(Estimate::of(20.0, 2000).upper, 0.015395938459075187, 1e-12);
    let half = Estimate::of(50.0, 100);
    assert_eq!((half.mean, half.n), (0.5, 100));
    assert_eq!(
        Estimate::of(0.0, 0),
        Estimate {
            mean: 0.0,
            upper: 1.0,
            n: 0
        }
    );
    let at = Estimate {
        mean: 0.0,
        upper: 0.05,
        n: 1,
    };
    assert!(at.holds(0.05));
    let above = Estimate {
        upper: 0.05f64.next_up(),
        ..at
    };
    assert!(!above.holds(0.05));
}

#[test]
fn wilson_clamps_the_mean_into_the_unit_interval() {
    let low = Estimate::of(-5.0, 100);
    assert_eq!(low.mean, -0.05);
    assert_eq!(low.upper, Estimate::of(0.0, 100).upper);
    let high = Estimate::of(150.0, 100);
    assert_eq!(high.mean, 1.5);
    close_to(high.upper, 1.0, 1e-12);
}

#[test]
fn single_trees_hold_fwer_and_fdr() {
    single_trees_hold(2_000);
}

#[test]
fn streams_hold_fdr() {
    streams_hold(500);
}

#[test]
fn the_suite_catches_undeclared_children() {
    let s = Scenario {
        non_null: 1.0,
        effect: 2.0,
        undeclared: DEFECT,
        ..base()
    };
    let bad = simulate(&s, 2_000, &mut seed(OTHER, 0)).unwrap();
    eprintln!("undeclared {DEFECT}: {bad:?}");
    assert!(!bad.fwer.holds(0.05), "{bad:?}");
    let good = simulate(&Scenario { undeclared: 0, ..s }, 2_000, &mut seed(OTHER, 1)).unwrap();
    eprintln!("undeclared 0: {good:?}");
    assert!(good.fwer.holds(0.05), "{good:?}");
}

#[test]
fn same_seed_same_report() {
    let s = &stream_grid()[1];
    let a = simulate(s, 50, &mut seed(OTHER, 2)).unwrap();
    let b = simulate(s, 50, &mut seed(OTHER, 2)).unwrap();
    assert_eq!(a, b);
    let c = simulate(s, 50, &mut seed(OTHER, 3)).unwrap();
    assert_ne!(a, c);
}

#[test]
fn truth_comes_from_the_node_means() {
    // Every declared node carries the effect, so no null hypothesis can be rejected.
    let all = Scenario {
        depth: 2,
        non_null: 1.0,
        effect: 3.0,
        root_only: false,
        ..base()
    };
    let r = simulate(&all, 200, &mut seed(OTHER, 4)).unwrap();
    assert_eq!((r.fwer.mean, r.fwer.n), (0.0, 200));
    assert_eq!((r.fdr.mean, r.fdr.n), (0.0, 200));
    assert_eq!((r.power.mean, r.power.n), (1.0, 200));

    // A tree that carries a zero effect is a null tree: it draws the same data and
    // gets the same report as a tree that carries none. Each null rejection makes it
    // a false discovery, so fdr is at least fwer.
    let none = Scenario {
        undeclared: DEFECT,
        ..base()
    };
    let zero = Scenario {
        non_null: 1.0,
        ..none.clone()
    };
    let r = simulate(&zero, 1_000, &mut seed(OTHER, 5)).unwrap();
    assert_eq!(r, simulate(&none, 1_000, &mut seed(OTHER, 5)).unwrap());
    assert_eq!(r.power, Estimate::of(0.0, 0));
    assert!(r.fwer.mean > 0.0 && r.fdr.mean >= r.fwer.mean, "{r:?}");
}

/// xoshiro256** outputs `rotl(s1 * 5, 7) * 9` from its second state word, so this
/// state makes the first `uniform()` exactly 1.
fn first_uniform_is_one() -> Rng {
    let inverse = |a: u64| {
        (0..6).fold(a, |x, _| {
            x.wrapping_mul(2u64.wrapping_sub(a.wrapping_mul(x)))
        })
    };
    let s1 = (u64::MAX.wrapping_mul(inverse(9)))
        .rotate_right(7)
        .wrapping_mul(inverse(5));
    let mut bytes = [0u8; 32];
    bytes[8..16].copy_from_slice(&s1.to_le_bytes());
    let rng = Rng::from_seed(bytes);
    assert_eq!(rng.clone().uniform(), 1.0);
    rng
}

#[test]
fn non_null_one_carries_even_at_a_draw_of_one() {
    let s = Scenario {
        non_null: 1.0,
        effect: 3.0,
        root_only: false,
        ..base()
    };
    let r = simulate(&s, 1, &mut first_uniform_is_one()).unwrap();
    assert_eq!(r.power.n, 1);
}

#[test]
fn spawned_children_share_the_tree_level() {
    // Both runs draw the same spawn values, so only the spawn differs. Spawned null
    // children cut the root's level to a quarter and dilute the tree's e-value.
    let s = Scenario {
        non_null: 1.0,
        effect: 1.0,
        ..base()
    };
    let never = Scenario {
        theta: f64::INFINITY,
        ..s.clone()
    };
    let always = Scenario {
        theta: 1.0f64.next_up(),
        ..s
    };
    let alone = simulate(&never, 2_000, &mut seed(OTHER, 8)).unwrap();
    let spawned = simulate(&always, 2_000, &mut seed(OTHER, 8)).unwrap();
    eprintln!(
        "alone {alone:?}
spawned {spawned:?}"
    );
    assert!(
        spawned.power.mean < 0.8 * alone.power.mean,
        "{alone:?} {spawned:?}"
    );
}

#[test]
fn a_small_mixed_scenario_is_pinned() {
    let s = Scenario {
        alpha: 0.5,
        gamma: Gamma::Bounded { m: 4 },
        trees: 4,
        children: 2,
        depth: 2,
        units: 20,
        rho: 0.5,
        theta: 2.0,
        non_null: 0.5,
        effect: 1.0,
        root_only: true,
        undeclared: 2,
    };
    // Regression pin: any change to the data model or the draw order moves these.
    let r = simulate(&s, 300, &mut seed(OTHER, 9)).unwrap();
    assert_eq!(
        (r.fwer.mean, r.fdr.mean, r.power.mean, r.power.n),
        (0.13, 0.01388888888888889, 0.7083333333333331, 280)
    );
}

#[test]
fn only_firing_parents_run_undeclared_children() {
    let s = Scenario {
        non_null: 1.0,
        effect: 2.0,
        undeclared: 10,
        ..base()
    };
    // Depth 0 has no triggers; an unreachable theta never fires. Only the root runs,
    // and it carries the effect.
    for quiet in [
        Scenario {
            depth: 0,
            ..s.clone()
        },
        Scenario {
            theta: f64::INFINITY,
            ..s.clone()
        },
    ] {
        let r = simulate(&quiet, 500, &mut seed(OTHER, 6)).unwrap();
        assert_eq!(r.fwer.mean, 0.0, "{quiet:?}");
    }
}

#[test]
fn rejects_bad_scenarios() {
    let ok = base();
    let cases = [
        (
            Scenario {
                alpha: 0.0,
                ..base()
            },
            "alpha",
        ),
        (
            Scenario {
                alpha: 1.0,
                ..base()
            },
            "alpha",
        ),
        (
            Scenario {
                alpha: f64::NAN,
                ..base()
            },
            "alpha",
        ),
        (
            Scenario {
                gamma: Gamma::Bounded { m: 0 },
                ..base()
            },
            "gamma",
        ),
        (Scenario { trees: 0, ..base() }, "trees"),
        (Scenario { depth: 3, ..base() }, "depth"),
        (Scenario { units: 0, ..base() }, "units"),
        (
            Scenario {
                rho: -0.1,
                ..base()
            },
            "rho",
        ),
        (Scenario { rho: 1.0, ..base() }, "rho"),
        (
            Scenario {
                rho: f64::NAN,
                ..base()
            },
            "rho",
        ),
        (
            Scenario {
                theta: 1.0,
                ..base()
            },
            "theta",
        ),
        (
            Scenario {
                theta: f64::NAN,
                ..base()
            },
            "theta",
        ),
        (
            Scenario {
                non_null: -0.1,
                ..base()
            },
            "non_null",
        ),
        (
            Scenario {
                non_null: 1.1,
                ..base()
            },
            "non_null",
        ),
        (
            Scenario {
                non_null: f64::NAN,
                ..base()
            },
            "non_null",
        ),
        (
            Scenario {
                effect: f64::INFINITY,
                ..base()
            },
            "effect",
        ),
        (
            Scenario {
                effect: f64::NAN,
                ..base()
            },
            "effect",
        ),
        (
            Scenario {
                children: usize::MAX,
                ..base()
            },
            "children",
        ),
    ];
    for (s, name) in cases {
        assert_eq!(
            simulate(&s, 1, &mut seed(OTHER, 7)),
            Err(Error::Parameter(name)),
            "{s:?}"
        );
    }
    assert_eq!(
        simulate(&ok, 0, &mut seed(OTHER, 7)),
        Err(Error::Parameter("reps"))
    );

    let edges = [
        ok.clone(),
        Scenario {
            alpha: 1e-9,
            ..base()
        },
        Scenario {
            alpha: 1.0f64.next_down(),
            ..base()
        },
        Scenario {
            gamma: Gamma::Unbounded,
            ..base()
        },
        Scenario { depth: 0, ..base() },
        Scenario { depth: 2, ..base() },
        Scenario { units: 1, ..base() },
        Scenario {
            rho: 1.0f64.next_down(),
            ..base()
        },
        Scenario {
            theta: 1.0f64.next_up(),
            ..base()
        },
        Scenario {
            theta: f64::INFINITY,
            ..base()
        },
        Scenario {
            non_null: 1.0,
            ..base()
        },
        Scenario {
            non_null: 1.0,
            effect: -f64::MAX,
            ..base()
        },
        Scenario {
            children: 0,
            ..base()
        },
        Scenario {
            children: usize::MAX,
            depth: 0,
            ..base()
        },
    ];
    for s in edges {
        assert!(simulate(&s, 1, &mut seed(OTHER, 7)).is_ok(), "{s:?}");
    }
}

#[test]
#[ignore]
fn null_tree_suite_full() {
    single_trees_hold(100_000);
    streams_hold(10_000);
}
