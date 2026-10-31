use ignorarium::machine::Machine;
use ignorarium::schedule::{self, Row, Schedule, ScheduleError, expand};
use ignorarium::seed::seed;
use ignorarium::spec::model::{Pick, Role, Spec};
use ignorarium::units::{self, Cluster};
use serde_json::{Value, json};
use std::path::Path;

const H: &str = "7ce680b26ed62ffb64d760b3842f3dd4b6272afee1b0d9c3050a489c62be0ab8";

fn hash() -> [u8; 32] {
    hex::decode(H).unwrap().try_into().unwrap()
}

fn template() -> Value {
    ignorarium::spec::load(Path::new("../../templates/numeric-ablation.toml"))
        .unwrap()
        .value
}

/// The template with `conds`, the confirmation stage running every one but the reference.
fn spec(conds: &[&str], reference: &str, repeats: u64) -> Spec {
    let mut v = template();
    v["design"]["conditions"] = Value::Array(conds.iter().map(|c| json!({"id": c})).collect());
    v["design"]["reference"] = json!(reference);
    v["design"]["repeats"] = json!(repeats);
    let others: Vec<_> = conds.iter().filter(|c| **c != reference).collect();
    v["design"]["confirmation"] = json!({ "conditions": others });
    Spec::from_value(&v).unwrap()
}

fn cluster(path: &str, stratum: &str, picked: &[u32]) -> Cluster {
    cluster_of(Role::Confirmation, path, stratum, picked)
}

fn cluster_of(role: Role, path: &str, stratum: &str, picked: &[u32]) -> Cluster {
    Cluster {
        path: path.into(),
        stratum: stratum.into(),
        role,
        picked: picked.to_vec(),
    }
}

/// Candidates `a` and `b` against the reference `exact` in a selection stage; the
/// confirmation stage runs the pick.
fn staged(repeats: u64) -> Spec {
    let mut v = template();
    v["design"]["conditions"] = json!([{ "id": "exact" }, { "id": "a" }, { "id": "b" }]);
    v["design"]["repeats"] = json!(repeats);
    v["design"]["selection"] =
        json!({ "metric": "ppl-ratio", "conditions": ["b", "a"], "rule": "best-mean" });
    v["design"]["confirmation"] = json!({ "conditions": ["selected"] });
    Spec::from_value(&v).unwrap()
}

/// A selection cluster in stratum `z` and confirmation clusters in strata that sort before it.
fn stage_clusters() -> Vec<Cluster> {
    vec![
        cluster("c2", "a", &[1]),
        cluster_of(Role::Selection, "s", "z", &[0, 4]),
        cluster("c1", "a", &[0]),
    ]
}

/// (stage, unit, condition, repeat) of each row.
fn stage_rows(rows: &[Row]) -> Vec<(&str, &str, &str, u32)> {
    rows.iter()
        .map(|r| (&r.stage[..], &r.unit[..], &r.condition[..], r.repeat))
        .collect()
}

#[test]
fn the_selection_stage_runs_its_candidates_on_selection_inputs_only() {
    let rows = expand(&staged(1), &hash(), &stage_clusters()).unwrap();
    // The selection stage comes first, whatever the strata; the reference, then the
    // candidates in the order the stage lists them.
    assert_eq!(
        stage_rows(&rows[..6]),
        [
            ("selection", "s#0", "exact", 0),
            ("selection", "s#0", "b", 0),
            ("selection", "s#0", "a", 0),
            ("selection", "s#4", "exact", 0),
            ("selection", "s#4", "b", 0),
            ("selection", "s#4", "a", 0),
        ]
    );
    assert!(rows[..6].iter().all(|r| r.cluster == "s"));
    assert!(rows[6..].iter().all(|r| r.stage == "confirmation"));
    for (i, r) in rows.iter().enumerate() {
        assert_eq!(r.index, i as u64);
        assert_eq!(r.seed_hex, hex::encode(seed(&hash(), &format!("row/{i}"))));
    }
}

#[test]
fn the_confirmation_stage_runs_selected_on_confirmation_inputs_only() {
    let rows = expand(&staged(2), &hash(), &stage_clusters()).unwrap();
    assert_eq!(rows.len(), 2 * 3 * 2 + 2 * 2 * 2);
    assert_eq!(
        stage_rows(&rows[12..]),
        [
            ("confirmation", "c1#0", "exact", 0),
            ("confirmation", "c1#0", "exact", 1),
            ("confirmation", "c1#0", "selected", 0),
            ("confirmation", "c1#0", "selected", 1),
            ("confirmation", "c2#1", "exact", 0),
            ("confirmation", "c2#1", "exact", 1),
            ("confirmation", "c2#1", "selected", 0),
            ("confirmation", "c2#1", "selected", 1),
        ]
    );
    // Without a selection stage, a selection input runs nothing.
    let rows = expand(
        &spec(&["exact", "a"], "exact", 1),
        &hash(),
        &stage_clusters(),
    )
    .unwrap();
    assert_eq!(rows.len(), 4);
    assert!(
        rows.iter()
            .all(|r| r.stage == "confirmation" && r.cluster != "s")
    );
}

#[test]
fn the_estimate_counts_each_stage() {
    // Two stages: 4 candidates on 8 texts, then the pick on 52 texts, 4 units per text.
    let mut v = template();
    let ids = ["exact", "c1", "c2", "c3", "c4"];
    v["design"]["conditions"] = json!(ids.map(|id| json!({ "id": id })));
    v["design"]["units_per_cluster"] = json!(4);
    v["design"]["selection"] =
        json!({ "metric": "ppl-ratio", "conditions": &ids[1..], "rule": "best-mean" });
    v["design"]["confirmation"] = json!({ "conditions": ["selected"] });
    let texts = |role: &str, n: usize| -> Vec<Value> {
        (0..n)
            .map(|i| input(&format!("data/{role}-{i:02}.txt"), role, 10))
            .collect()
    };
    let mut inputs = texts("selection", 8);
    inputs.extend(texts("confirmation", 52));
    v["inputs"] = json!(inputs);
    let s = Spec::from_value(&v).unwrap();
    let rows = schedule::rows(&s, &hash()).unwrap();
    let e = ignorarium::estimate::estimate(&s, &Machine::default(), &rows);
    // 5 conditions x 8 texts x 4 units, then 2 x 52 x 4.
    assert_eq!(e.rows, 5 * 8 * 4 + 2 * 52 * 4);
    assert_eq!(e.rows, 576);
    let per: Vec<(&str, u64)> = e
        .rows_per_condition
        .iter()
        .map(|(id, n)| (id.as_str(), *n))
        .collect();
    assert_eq!(
        per,
        [
            ("exact", 32 + 208),
            ("c1", 32),
            ("c2", 32),
            ("c3", 32),
            ("c4", 32),
            ("selected", 208)
        ]
    );
}

/// One cluster `c` whose units are 0..n.
fn units(n: u32) -> Vec<Cluster> {
    vec![cluster("c", "s", &(0..n).collect::<Vec<_>>())]
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
            ("c#0", "ref", 0),
            ("c#0", "ref", 1),
            ("c#0", "a", 0),
            ("c#0", "a", 1)
        ]
    );
    assert_eq!(rows[4].unit, "c#1");
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
fn rows_name_units_by_path_and_index() {
    let s = spec(&["r"], "r", 1);
    let rows = expand(&s, &hash(), &[cluster("data/a.txt", "code", &[3, 7])]).unwrap();
    let got: Vec<_> = rows
        .iter()
        .map(|r| (&r.unit[..], &r.cluster[..], &r.stratum[..]))
        .collect();
    assert_eq!(
        got,
        [
            ("data/a.txt#3", "data/a.txt", "code"),
            ("data/a.txt#7", "data/a.txt", "code")
        ]
    );
}

#[test]
fn clusters_sort_by_stratum_then_path_and_their_order_is_irrelevant() {
    let s = spec(&["r"], "r", 1);
    let a = vec![
        cluster("c2", "s1", &[0]),
        cluster("c1", "s1", &[1, 4]),
        cluster("c9", "s0", &[2]),
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
            ("s0", "c9", "c9#2"),
            ("s1", "c1", "c1#1"),
            ("s1", "c1", "c1#4"),
            ("s1", "c2", "c2#0")
        ]
    );
}

fn input(path: &str, role: &str, units: u32) -> Value {
    json!({
        "path": path, "sha256": "", "role": role, "stratum": "fiction", "units": units,
        "licence": { "spdx": "", "type": "unspecified", "source": "", "creators": [] }
    })
}

#[test]
fn the_schedule_of_a_spec_holds_each_clusters_picked_units() {
    let mut v: Value = ignorarium::spec::load(Path::new("../../templates/numeric-ablation.toml"))
        .unwrap()
        .value;
    v["design"]["units_per_cluster"] = json!(5);
    v["design"]["repeats"] = json!(2);
    v["design"]["conditions"] = json!([{ "id": "exact" }, { "id": "a" }, { "id": "b" }]);
    v["design"]["selection"] =
        json!({ "metric": "ppl-ratio", "conditions": ["a", "b"], "rule": "best-mean" });
    v["design"]["confirmation"] = json!({ "conditions": ["a", "b"] });
    v["inputs"] = json!([
        input("data/cal.txt", "calibration", 40),
        input("data/s.txt", "selection", 2),
        input("data/c.txt", "confirmation", 7),
    ]);
    let s = Spec::from_value(&v).unwrap();
    let rows = schedule::rows(&s, &hash()).unwrap();
    assert_eq!(
        rows,
        expand(&s, &hash(), &units::clusters(&s, &hash())).unwrap()
    );
    // (2 + 5 units) × 3 conditions × 2 repeats: the count the row cap rule computes.
    assert_eq!(rows.len(), 42);
    let picked = units::pick(5, 7, Pick::Seeded, seed(&hash(), "units/data/c.txt"));
    let want: Vec<String> = ["data/s.txt#0".to_string(), "data/s.txt#1".into()]
        .into_iter()
        .chain(picked.iter().map(|i| format!("data/c.txt#{i}")))
        .collect();
    let mut got: Vec<String> = rows.iter().map(|r| r.unit.clone()).collect();
    got.dedup();
    assert_eq!(got, want);
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

fn sched(n: u32) -> Schedule {
    let rows: Vec<Row> = expand(&spec(&["r"], "r", 1), &hash(), &units(n)).unwrap();
    Schedule::new(&hash(), rows)
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
    assert_eq!(v["spec_hash"], format!("sha256:{H}"));
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

#[test]
fn schedule_spec_hash_matches_the_raw_spec_hash() {
    let raw = ignorarium::spec::load(Path::new("../../templates/numeric-ablation.toml")).unwrap();
    let digest: [u8; 32] = hex::decode(raw.hash.strip_prefix("sha256:").unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(Schedule::new(&digest, vec![]).spec_hash, raw.hash);
}
