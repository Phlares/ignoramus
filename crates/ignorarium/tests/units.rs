use evidence::rng::Rng;
use ignorarium::seed::seed;
use ignorarium::spec::model::{Pick, Role, Spec};
use ignorarium::units::{self, Cluster, pick};
use serde_json::{Value, json};
use std::path::Path;

const H: &str = "7ce680b26ed62ffb64d760b3842f3dd4b6272afee1b0d9c3050a489c62be0ab8";

fn hash() -> [u8; 32] {
    hex::decode(H).unwrap().try_into().unwrap()
}

fn seed_of(path: &str) -> [u8; 32] {
    seed(&hash(), &format!("units/{path}"))
}

fn sorted(mut v: Vec<u32>) -> Vec<u32> {
    v.sort_unstable();
    v
}

#[test]
fn seeded_pick_is_fixed_by_spec_hash_and_path() {
    let a = pick(4, 31, Pick::Seeded, seed_of("data/a.txt"));
    assert_eq!(a, pick(4, 31, Pick::Seeded, seed_of("data/a.txt")));
    assert_ne!(a, pick(4, 31, Pick::Seeded, seed_of("data/b.txt")));
    let mut other = hash();
    other[0] ^= 1;
    let elsewhere = seed(&other, "units/data/a.txt");
    assert_ne!(a, pick(4, 31, Pick::Seeded, elsewhere));
    // Four distinct indices below 31, ascending.
    assert_eq!(a.len(), 4);
    assert!(a.windows(2).all(|w| w[0] < w[1]), "{a:?}");
    assert!(a.iter().all(|i| *i < 31), "{a:?}");
    // The rule: the first 4 positions of the seeded shuffle of 0..31, sorted.
    let drawn = Rng::from_seed(seed_of("data/a.txt")).sample(31, 4);
    let want = sorted(drawn.into_iter().map(|i| i as u32).collect());
    assert_eq!(a, want);
    assert_ne!(a, [0, 1, 2, 3]);
}

#[test]
fn a_cluster_with_fewer_units_gives_all_it_has() {
    for how in [Pick::Seeded, Pick::First] {
        assert_eq!(pick(10, 3, how, seed_of("data/a.txt")), [0, 1, 2]);
        assert_eq!(pick(3, 3, how, seed_of("data/a.txt")), [0, 1, 2]);
        assert!(pick(5, 0, how, seed_of("data/a.txt")).is_empty());
        assert!(pick(0, 5, how, seed_of("data/a.txt")).is_empty());
    }
}

#[test]
fn first_pick_takes_indices_from_zero() {
    for path in ["data/a.txt", "data/b.txt"] {
        assert_eq!(pick(4, 31, Pick::First, seed_of(path)), [0, 1, 2, 3]);
    }
    assert_eq!(pick(4, u32::MAX, Pick::First, [0; 32]), [0, 1, 2, 3]);
}

#[test]
fn a_seeded_pick_from_the_largest_count_stays_small() {
    let got = pick(3, u32::MAX, Pick::Seeded, seed_of("data/a.txt"));
    assert_eq!(got.len(), 3);
    assert!(got.windows(2).all(|w| w[0] < w[1]), "{got:?}");
}

#[test]
fn unit_keys_join_the_path_and_the_index() {
    assert_eq!(units::key("data/a.txt", 3), "data/a.txt#3");
    assert_eq!(units::key("x", 0), "x#0");
}

fn input(path: &str, role: &str, stratum: &str, units: Option<u32>) -> Value {
    let mut i = json!({
        "path": path, "sha256": "", "role": role, "stratum": stratum,
        "licence": { "spdx": "", "type": "unspecified", "source": "", "creators": [] }
    });
    if let Some(n) = units {
        i["units"] = json!(n);
    }
    i
}

fn spec(pick_rule: &str) -> Spec {
    let mut v = ignorarium::spec::load(Path::new("../../templates/numeric-ablation.toml"))
        .unwrap()
        .value;
    v["design"]["units_per_cluster"] = json!(3);
    v["design"]["pick"] = json!(pick_rule);
    v["inputs"] = json!([
        input("data/cal.txt", "calibration", "", Some(9)),
        input("data/s.txt", "selection", "poetry", Some(2)),
        input("models/m.bin", "model", "", None),
        input("data/c.txt", "confirmation", "fiction", Some(30)),
    ]);
    Spec::from_value(&v).unwrap()
}

#[test]
fn clusters_are_the_selection_and_confirmation_inputs_in_spec_order() {
    let got = units::clusters(&spec("seeded"), &hash());
    // Spec order, not path order: data/s.txt comes first.
    assert_eq!(
        got,
        [
            Cluster {
                path: "data/s.txt".into(),
                stratum: "poetry".into(),
                role: Role::Selection,
                picked: vec![0, 1],
            },
            Cluster {
                path: "data/c.txt".into(),
                stratum: "fiction".into(),
                role: Role::Confirmation,
                picked: pick(3, 30, Pick::Seeded, seed_of("data/c.txt")),
            },
        ]
    );
    let first = units::clusters(&spec("first"), &hash());
    assert_eq!(first[1].picked, [0, 1, 2]);
    assert_ne!(got[1].picked, first[1].picked);
}

#[test]
fn an_input_without_units_gives_no_units() {
    let mut s = spec("seeded");
    s.inputs[3].units = None;
    assert!(units::clusters(&s, &hash())[1].picked.is_empty());
}
