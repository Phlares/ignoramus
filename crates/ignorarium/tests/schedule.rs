use ignorarium::schedule::{self, Row, Schedule, ScheduleError, expand};
use ignorarium::seed::seed;
use ignorarium::spec::model::Spec;
use serde_json::{Value, json};
use std::path::Path;

const H: &str = "7ce680b26ed62ffb64d760b3842f3dd4b6272afee1b0d9c3050a489c62be0ab8";

fn hash() -> [u8; 32] {
    hex::decode(H).unwrap().try_into().unwrap()
}

fn spec(conds: &[&str], reference: &str, repeats: u64) -> Spec {
    let mut v: Value = ignorarium::spec::load(Path::new("../../templates/numeric-ablation.toml"))
        .unwrap()
        .value;
    v["design"]["conditions"] = Value::Array(conds.iter().map(|c| json!({"id": c})).collect());
    v["design"]["reference"] = json!(reference);
    v["design"]["repeats"] = json!(repeats);
    Spec::from_value(&v).unwrap()
}

fn u(unit: &str, cluster: &str, stratum: &str) -> (String, String, String) {
    (unit.into(), cluster.into(), stratum.into())
}

fn units(n: usize) -> Vec<(String, String, String)> {
    (0..n).map(|i| u(&format!("u{i}"), "c", "s")).collect()
}

#[test]
fn seed_known_vector() {
    assert_eq!(
        hex::encode(seed(&hash(), "row/0")),
        "fa5756770dff6ffcafe01f76a66a5afbe23765faefd65eeabee1a9d845514ce9"
    );
}

#[test]
fn seed_depends_on_hash_and_path() {
    let mut other = hash();
    other[0] ^= 1;
    assert_ne!(seed(&hash(), "row/0"), seed(&hash(), "row/1"));
    assert_ne!(seed(&hash(), "row/0"), seed(&other, "row/0"));
}

#[test]
fn expansion_count_and_order() {
    let s = spec(&["a", "ref"], "ref", 2);
    let rows = expand(&s, &hash(), &units(3)).unwrap();
    assert_eq!(rows.len(), 12);
    assert_eq!(rows[0].condition, "ref");
    let first: Vec<_> = rows[..4]
        .iter()
        .map(|r| (r.unit.as_str(), r.condition.as_str(), r.repeat))
        .collect();
    assert_eq!(
        first,
        [
            ("u0", "ref", 0),
            ("u0", "ref", 1),
            ("u0", "a", 0),
            ("u0", "a", 1)
        ]
    );
    assert_eq!(rows[4].unit, "u1");
    for (i, r) in rows.iter().enumerate() {
        assert_eq!(r.index, i as u64);
        assert_eq!(r.shard, 0);
    }
}

#[test]
fn row_count_for_second_shape() {
    let s = spec(&["a", "b", "c"], "b", 1);
    let rows = expand(&s, &hash(), &units(2)).unwrap();
    assert_eq!(rows.len(), 6);
    let conds: Vec<_> = rows[..3].iter().map(|r| r.condition.as_str()).collect();
    assert_eq!(conds, ["b", "a", "c"]);
}

#[test]
fn units_sort_by_stratum_cluster_unit_and_input_order_is_irrelevant() {
    let s = spec(&["r"], "r", 1);
    let a = vec![
        u("u2", "c1", "s1"),
        u("u1", "c2", "s1"),
        u("u3", "c1", "s0"),
        u("u1", "c1", "s1"),
    ];
    let mut b = a.clone();
    b.reverse();
    let ra = expand(&s, &hash(), &a).unwrap();
    assert_eq!(ra, expand(&s, &hash(), &b).unwrap());
    let order: Vec<_> = ra
        .iter()
        .map(|r| (&r.stratum[..], &r.cluster[..], &r.unit[..]))
        .collect();
    assert_eq!(
        order,
        [
            ("s0", "c1", "u3"),
            ("s1", "c1", "u1"),
            ("s1", "c1", "u2"),
            ("s1", "c2", "u1")
        ]
    );
}

#[test]
fn row_seeds_follow_the_row_path() {
    let s = spec(&["r"], "r", 1);
    let rows = expand(&s, &hash(), &units(2)).unwrap();
    assert_eq!(rows[0].seed_hex, hex::encode(seed(&hash(), "row/0")));
    assert_eq!(rows[1].seed_hex, hex::encode(seed(&hash(), "row/1")));
    assert_ne!(rows[0].seed_hex, rows[1].seed_hex);
}

#[test]
fn unknown_reference_is_an_error() {
    let s = spec(&["a", "b"], "zzz", 1);
    let e = expand(&s, &hash(), &units(1)).unwrap_err();
    assert_eq!(e.to_string(), "design.reference `zzz` names no condition");
}

#[test]
fn huge_repeats_with_no_units_do_not_overflow() {
    let s = spec(&["a", "b"], "a", u32::MAX as u64);
    assert!(expand(&s, &hash(), &[]).unwrap().is_empty());
}

fn sched(n: usize) -> Schedule {
    let rows: Vec<Row> = expand(&spec(&["r"], "r", 1), &hash(), &units(n)).unwrap();
    Schedule::new(H.into(), rows)
}

#[test]
fn schedule_write_once_refuses_a_different_schedule() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (sched(1), sched(2));
    schedule::write_once(dir.path(), &a).unwrap();
    schedule::write_once(dir.path(), &a).unwrap();
    let before = std::fs::read(dir.path().join("schedule.json")).unwrap();
    assert!(matches!(
        schedule::write_once(dir.path(), &b),
        Err(ScheduleError::Differs)
    ));
    assert_eq!(
        std::fs::read(dir.path().join("schedule.json")).unwrap(),
        before
    );
    assert!(!dir.path().join("schedule.json.tmp").exists());
}

#[test]
fn schedule_file_is_canonical_json_with_one_trailing_lf() {
    let dir = tempfile::tempdir().unwrap();
    let s = sched(1);
    schedule::write_once(dir.path(), &s).unwrap();
    let bytes = std::fs::read(dir.path().join("schedule.json")).unwrap();
    assert_eq!(bytes.last(), Some(&b'\n'));
    assert_ne!(bytes[bytes.len() - 2], b'\n');
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["box_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["spec_hash"], H);
    assert_eq!(v["rows"][0]["seed_hex"], s.rows[0].seed_hex);
    let mut canon = ignorarium::canon::to_canonical(&v).unwrap();
    canon.push(b'\n');
    assert_eq!(bytes, canon);
}

#[cfg(unix)]
#[test]
fn symlinked_schedule_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("elsewhere");
    std::fs::write(&target, b"x").unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join("schedule.json")).unwrap();
    assert!(matches!(
        schedule::write_once(dir.path(), &sched(1)),
        Err(ScheduleError::Symlink)
    ));
    assert_eq!(std::fs::read(&target).unwrap(), b"x");
}
