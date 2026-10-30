use evidence::claim::{Bound, Statistic, e_value, order};
use evidence::rng::Rng;
use evidence::{Tail, tree};
use ignorarium::analysis::{
    Paired, distinct_clusters, estimates, paired, select, study_e_value, verdicts,
};
use ignorarium::derived::{BoundResult, Candidate, Estimate, Selection, Verdict, contrast_key};
use ignorarium::home::Home;
use ignorarium::keys;
use ignorarium::records::Record;
use ignorarium::seed::seed;
use ignorarium::spec::model::{Direction, Role, SelectionRule, Spec};
use ignorarium::stream::{Log, LogName};
use ignorarium::units::Cluster;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn spec(kind: &str) -> Spec {
    let mut v = ignorarium::spec::load("../../templates/numeric-ablation.toml".as_ref())
        .unwrap()
        .value;
    v["outcomes"][0]["measure"] = json!("ppl");
    v["outcomes"][0]["paired"] = json!(kind);
    v["design"]["reference"] = json!("base");
    Spec::from_value(&v).unwrap()
}

fn row(unit: &str, cond: &str, repeat: u32, exit: &str, ppl: Option<f64>) -> Record {
    let mut body = json!({ "condition": cond, "unit": unit, "repeat": repeat, "exit": exit });
    if let Some(p) = ppl {
        body["values"] = json!({ "ppl": p });
    }
    Record {
        seq: 0,
        prev: String::new(),
        study: "s".into(),
        row: Some(0),
        kind: "row".into(),
        body,
        ts: String::new(),
    }
}

fn ok(unit: &str, cond: &str, repeat: u32, ppl: f64) -> Record {
    row(unit, cond, repeat, "ok", Some(ppl))
}

#[test]
fn percent_pairs_by_unit_and_repeat() {
    let rows = [
        ok("a#0", "base", 0, 100.0),
        ok("a#0", "new", 0, 110.0),
        ok("a#1", "base", 0, 50.0),
        ok("a#1", "new", 0, 40.0),
    ];
    let p = paired(&spec("percent"), &rows);
    let got = p.get("a#0", "new", "ppl-ratio").unwrap();
    assert!((got - 10.0).abs() < 1e-9, "{got}");
    let got = p.get("a#1", "new", "ppl-ratio").unwrap();
    assert!((got + 20.0).abs() < 1e-9, "{got}");
    // The reference against itself pairs to zero; an unknown outcome has no value.
    assert_eq!(p.get("a#0", "base", "ppl-ratio"), Some(0.0));
    assert_eq!(p.get("a#0", "new", "other"), None);
    assert_eq!(p.units().collect::<Vec<_>>(), ["a#0", "a#1"]);
}

#[test]
fn difference_is_condition_minus_reference_and_repeats_average() {
    let rows = [
        ok("a#0", "base", 0, 100.0),
        ok("a#0", "new", 0, 90.0),
        ok("a#0", "base", 1, 100.0),
        ok("a#0", "new", 1, 70.0),
        // Repeat 2 has no reference row: it does not pair.
        ok("a#0", "new", 2, 0.0),
    ];
    let p = paired(&spec("difference"), &rows);
    assert_eq!(p.get("a#0", "new", "ppl-ratio"), Some(-20.0));
}

#[test]
fn a_unit_without_its_reference_row_has_no_paired_value() {
    let rows = [
        ok("a#0", "new", 0, 110.0),
        ok("a#1", "base", 0, 100.0),
        // A void reference row and a payload failure do not count.
        ok("a#2", "new", 0, 110.0),
        row("a#2", "base", 0, "void:output", None),
        ok("a#3", "base", 0, 100.0),
        row("a#3", "new", 0, "payload:1", None),
        // A record of another kind never counts.
        Record {
            kind: "spend".into(),
            ..ok("a#4", "new", 0, 1.0)
        },
        ok("a#4", "base", 0, 1.0),
    ];
    let p = paired(&spec("percent"), &rows);
    for (u, c) in [
        ("a#0", "new"),
        ("a#2", "new"),
        ("a#3", "new"),
        ("a#4", "new"),
    ] {
        assert_eq!(p.get(u, c, "ppl-ratio"), None, "{u}");
    }
}

#[test]
fn a_zero_reference_has_no_percent_value() {
    let rows = [
        ok("a#0", "base", 0, 0.0),
        ok("a#0", "new", 0, 5.0),
        ok("a#1", "base", 0, -1.0),
        ok("a#1", "new", 0, 5.0),
        ok("a#2", "base", 0, 1e-9),
        ok("a#2", "new", 0, 2e-9),
    ];
    let p = paired(&spec("percent"), &rows);
    assert_eq!(p.get("a#0", "new", "ppl-ratio"), None);
    assert_eq!(p.get("a#1", "new", "ppl-ratio"), None);
    let tiny = p.get("a#2", "new", "ppl-ratio").unwrap();
    assert!((tiny - 100.0).abs() < 1e-6, "{tiny}");
    // The difference form has no such limit.
    let d = paired(&spec("difference"), &rows);
    assert_eq!(d.get("a#0", "new", "ppl-ratio"), Some(5.0));
    assert_eq!(d.get("a#1", "new", "ppl-ratio"), Some(6.0));
}

#[test]
fn log_ratio_is_the_log_of_condition_over_reference() {
    let rows = [
        ok("a#0", "base", 0, 100.0),
        ok("a#0", "new", 0, 104.0),
        ok("a#1", "base", 0, 50.0),
        ok("a#1", "new", 0, 25.0),
        // Repeats average on the log scale: ln 2 and ln 0.5 give 0.
        ok("a#2", "base", 0, 10.0),
        ok("a#2", "new", 0, 20.0),
        ok("a#2", "base", 1, 10.0),
        ok("a#2", "new", 1, 5.0),
    ];
    let p = paired(&spec("log-ratio"), &rows);
    let got = p.get("a#0", "new", "ppl-ratio").unwrap();
    assert!((got - 1.04_f64.ln()).abs() < 1e-12, "{got}");
    let got = p.get("a#1", "new", "ppl-ratio").unwrap();
    assert!((got + 2.0_f64.ln()).abs() < 1e-12, "{got}");
    let got = p.get("a#2", "new", "ppl-ratio").unwrap();
    assert!(got.abs() < 1e-12, "{got}");
    assert_eq!(p.get("a#0", "base", "ppl-ratio"), Some(0.0));
}

#[test]
fn a_non_positive_side_has_no_log_ratio() {
    let rows = [
        ok("a#0", "base", 0, 0.0),
        ok("a#0", "new", 0, 5.0),
        ok("a#1", "base", 0, -1.0),
        ok("a#1", "new", 0, -5.0),
        ok("a#2", "base", 0, 5.0),
        ok("a#2", "new", 0, 0.0),
        ok("a#3", "base", 0, 5.0),
        ok("a#3", "new", 0, -1.0),
        // Both sides just above zero still pair.
        ok("a#4", "base", 0, 1e-300),
        ok("a#4", "new", 0, 1e-300),
    ];
    let p = paired(&spec("log-ratio"), &rows);
    for u in ["a#0", "a#1", "a#2", "a#3"] {
        assert_eq!(p.get(u, "new", "ppl-ratio"), None, "{u}");
    }
    assert_eq!(p.get("a#4", "new", "ppl-ratio"), Some(0.0));
}

#[test]
fn a_pair_that_overflows_is_absent() {
    let rows = [
        ok("a#0", "base", 0, 1e-308),
        ok("a#0", "new", 0, 1e308),
        ok("a#1", "base", 0, -1e308),
        ok("a#1", "new", 0, 1e308),
    ];
    assert_eq!(
        paired(&spec("percent"), &rows).get("a#0", "new", "ppl-ratio"),
        None
    );
    let d = paired(&spec("difference"), &rows);
    assert_eq!(d.get("a#1", "new", "ppl-ratio"), None);
    // The mean of finite pairs that sum past the range is absent too.
    let rows = [
        ok("a#2", "base", 0, -0.5e308),
        ok("a#2", "new", 0, 1e308),
        ok("a#2", "base", 1, -0.5e308),
        ok("a#2", "new", 1, 1e308),
    ];
    let d = paired(&spec("difference"), &rows);
    assert_eq!(d.get("a#2", "new", "ppl-ratio"), None);
}

/// A selection stage over `candidates`, against the reference `base`, ranking the paired
/// difference of the template's one outcome in `direction`.
fn staged(candidates: &[&str], direction: &str) -> Spec {
    let mut v = ignorarium::spec::load("../../templates/numeric-ablation.toml".as_ref())
        .unwrap()
        .value;
    v["outcomes"][0]["paired"] = json!("difference");
    v["outcomes"][0]["direction"] = json!(direction);
    v["design"]["reference"] = json!("base");
    v["design"]["selection"] =
        json!({ "metric": "ppl-ratio", "conditions": candidates, "rule": "best-mean" });
    v["design"]["confirmation"] = json!({ "conditions": ["selected"] });
    Spec::from_value(&v).unwrap()
}

/// `r` as a row of `stage`.
fn in_stage(stage: &str, mut r: Record) -> Record {
    r.body["stage"] = json!(stage);
    r
}

/// An `ok` selection-stage row.
fn sel(unit: &str, cond: &str, ppl: f64) -> Record {
    in_stage("selection", ok(unit, cond, 0, ppl))
}

/// A selection-stage row that recorded no values.
fn sel_failed(unit: &str, cond: &str, exit: &str) -> Record {
    in_stage("selection", row(unit, cond, 0, exit, None))
}

fn candidates(s: &Selection) -> Vec<(&str, f64)> {
    s.candidates
        .iter()
        .map(|c| (c.id.as_str(), c.mean))
        .collect()
}

#[test]
fn select_picks_the_best_mean_by_the_outcome_direction() {
    let rows = [
        sel("s#0", "base", 100.0),
        sel("s#0", "a", 110.0),
        sel("s#0", "b", 105.0),
        sel("s#1", "base", 100.0),
        sel("s#1", "a", 90.0),
        sel("s#1", "b", 101.0),
    ];
    let lower = select(&staged(&["a", "b"], "lower"), &rows).unwrap();
    assert_eq!(
        lower,
        Selection {
            metric: "ppl-ratio".into(),
            rule: SelectionRule::BestMean,
            direction: Direction::Lower,
            candidates: vec![
                Candidate {
                    id: "a".into(),
                    mean: 0.0
                },
                Candidate {
                    id: "b".into(),
                    mean: 3.0
                },
            ],
            selected: "a".into(),
            units: vec!["s#0".into(), "s#1".into()],
            complete_units: 2,
            incomplete_units: 0,
        }
    );
    let higher = select(&staged(&["a", "b"], "higher"), &rows).unwrap();
    assert_eq!(higher.selected, "b");
    assert_eq!(higher.direction, Direction::Higher);
    assert_eq!(candidates(&higher), [("a", 0.0), ("b", 3.0)]);
}

#[test]
fn a_tie_goes_to_the_first_listed() {
    let rows = [
        sel("s#0", "base", 10.0),
        sel("s#0", "a", 12.0),
        sel("s#0", "b", 11.0),
        sel("s#1", "base", 10.0),
        sel("s#1", "a", 12.0),
        sel("s#1", "b", 13.0),
    ];
    for direction in ["lower", "higher"] {
        for (order, first) in [(["a", "b"], "a"), (["b", "a"], "b")] {
            let s = select(&staged(&order, direction), &rows).unwrap();
            assert_eq!(s.selected, first, "{direction} {order:?}");
            assert_eq!(s.candidates[0].mean, s.candidates[1].mean);
        }
    }
}

#[test]
fn incomplete_units_stay_out_of_the_pick() {
    let rows = [
        // No value for b: out of the pick, though a is best here.
        sel("s#0", "base", 100.0),
        sel("s#0", "a", 0.0),
        sel_failed("s#0", "b", "void:output"),
        // A void reference row pairs nothing.
        sel_failed("s#1", "base", "void:output"),
        sel("s#1", "a", 1.0),
        sel("s#1", "b", 1.0),
        sel("s#2", "base", 100.0),
        sel("s#2", "a", 105.0),
        sel("s#2", "b", 101.0),
        // Confirmation rows are not selection units, whatever their condition.
        in_stage("confirmation", ok("c#0", "base", 0, 100.0)),
        in_stage("confirmation", ok("c#0", "a", 0, 0.0)),
    ];
    let s = select(&staged(&["a", "b"], "lower"), &rows).unwrap();
    assert_eq!(candidates(&s), [("a", 5.0), ("b", 1.0)]);
    assert_eq!(s.selected, "b");
    assert_eq!((s.complete_units, s.incomplete_units), (1, 2));
    assert_eq!(s.units, ["s#2"]);
}

#[test]
fn select_refuses_without_a_complete_unit_a_stage_or_a_finite_mean() {
    let spec = staged(&["a", "b"], "lower");
    let rows = [
        sel("s#0", "base", 100.0),
        sel("s#0", "a", 90.0),
        sel_failed("s#0", "b", "payload:2"),
        sel("s#1", "base", 100.0),
        sel_failed("s#1", "a", "void:output"),
    ];
    assert_eq!(
        select(&spec, &rows).unwrap_err(),
        "no selection unit has a ppl-ratio value for every candidate; incomplete units: 2"
    );
    assert_eq!(
        select(&spec, &[]).unwrap_err(),
        "no selection unit has a ppl-ratio value for every candidate; incomplete units: 0"
    );
    let mut plain = spec.clone();
    plain.design.selection = None;
    assert_eq!(
        select(&plain, &rows).unwrap_err(),
        "the plan has no selection stage"
    );
    let mut other = spec.clone();
    other.design.selection.as_mut().unwrap().metric = "nope".into();
    assert_eq!(
        select(&other, &rows).unwrap_err(),
        "the selection metric is not an outcome of the plan"
    );
    let mut none = spec.clone();
    none.design.selection.as_mut().unwrap().conditions.clear();
    assert_eq!(
        select(&none, &rows).unwrap_err(),
        "the selection stage lists no candidate"
    );
    // Two finite pairs whose sum overflows.
    let big = [
        sel("s#0", "base", 0.0),
        sel("s#0", "a", 1e308),
        sel("s#0", "b", 0.0),
        sel("s#1", "base", 0.0),
        sel("s#1", "a", 1e308),
        sel("s#1", "b", 0.0),
    ];
    assert_eq!(
        select(&spec, &big).unwrap_err(),
        "the mean ppl-ratio of candidate a is not finite"
    );
}

#[test]
fn a_repeat_with_a_zero_reference_drops_out_of_the_percent_mean() {
    let rows = [
        ok("a#0", "base", 0, 0.0),
        ok("a#0", "new", 0, 5.0),
        ok("a#0", "base", 1, 100.0),
        ok("a#0", "new", 1, 120.0),
    ];
    let p = paired(&spec("percent"), &rows);
    let got = p.get("a#0", "new", "ppl-ratio").unwrap();
    assert!((got - 20.0).abs() < 1e-9, "{got}");
}

// ---- estimates ----

/// The template's spec, reference `base`, difference-paired, with `strata` on its hypothesis.
fn one_hypothesis(strata: &[&str]) -> Spec {
    let mut v = ignorarium::spec::load("../../templates/numeric-ablation.toml".as_ref())
        .unwrap()
        .value;
    v["outcomes"][0]["paired"] = json!("difference");
    v["design"]["reference"] = json!("base");
    v["question"]["hypotheses"][0]["strata"] = json!(strata);
    Spec::from_value(&v).unwrap()
}

/// A confirmation cluster whose units `<path>#0`, `<path>#1`, ... are all picked.
fn cluster(path: &str, stratum: &str, units: u32) -> Cluster {
    Cluster {
        path: path.into(),
        stratum: stratum.into(),
        role: Role::Confirmation,
        picked: (0..units).collect(),
    }
}

/// `arm-a` rows that pair to `x` against `base`, for the units `<path>#i`.
fn arm_a_rows(path: &str, xs: &[f64]) -> Vec<Record> {
    xs.iter()
        .enumerate()
        .flat_map(|(i, x)| {
            let u = format!("{path}#{i}");
            [ok(&u, "base", 0, 10.0), ok(&u, "arm-a", 0, 10.0 + x)]
        })
        .collect()
}

fn no_params() -> BTreeMap<String, serde_json::Value> {
    BTreeMap::new()
}

/// The pooled estimates of the one hypothesis over `groups` (one cluster per slice).
fn pooled(groups: &[&[f64]]) -> Vec<Estimate> {
    let spec = one_hypothesis(&[]);
    let mut rows = Vec::new();
    let mut clusters = Vec::new();
    for (i, g) in groups.iter().enumerate() {
        let path = format!("c{i}");
        rows.extend(arm_a_rows(&path, g));
        clusters.push(cluster(&path, "", g.len() as u32));
    }
    let p = paired(&spec, &rows);
    estimates(&spec, &no_params(), &p, &clusters, None)
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} != {b}");
}

#[test]
fn anova_matches_a_hand_computed_case() {
    // Cluster means 2.5, 4.5, 6.5; MSB = 16, MSW = 15 / 9, m0 = 4:
    // sigma_b^2 = 43 / 12, var = 63 / 12 = 5.25, icc = 43 / 63.
    let e = pooled(&[
        &[1.0, 2.0, 3.0, 4.0],
        &[3.0, 4.0, 5.0, 6.0],
        &[5.0, 6.0, 7.0, 8.0],
    ]);
    assert_eq!(e.len(), 1);
    let e = &e[0];
    assert_eq!((e.clusters, e.units), (3, 12));
    close(e.mean, 4.5);
    close(e.var, 5.25);
    close(e.icc.unwrap(), 43.0 / 63.0);
    assert_eq!(
        (
            e.hypothesis.as_str(),
            e.outcome.as_str(),
            e.stratum.as_str()
        ),
        ("H1", "ppl-ratio", "")
    );
    // Unequal clusters: m0 = 4 / 3, sigma_b^2 = 3, var = 5, icc = 0.6, and the mean is the
    // unweighted mean of the cluster means (3.5), not of the units (3).
    let e = &pooled(&[&[1.0, 3.0], &[5.0]])[0];
    assert_eq!((e.clusters, e.units), (2, 3));
    close(e.mean, 3.5);
    close(e.var, 5.0);
    close(e.icc.unwrap(), 0.6);
}

#[test]
fn a_negative_between_variance_counts_as_zero() {
    // Equal cluster means: MSB = 0 < MSW = 2, so sigma_b^2 = 0, var = MSW, icc = 0.
    let e = &pooled(&[&[1.0, 3.0], &[1.0, 3.0]])[0];
    close(e.var, 2.0);
    assert_eq!(e.icc, Some(0.0));
    // No variance at all: no icc.
    let e = &pooled(&[&[2.0, 2.0], &[2.0, 2.0]])[0];
    assert_eq!((e.var, e.icc), (0.0, None));
}

#[test]
fn one_cluster_has_no_icc() {
    // The sample variance of all x: 5 / 3.
    let e = &pooled(&[&[1.0, 2.0, 3.0, 4.0]])[0];
    assert_eq!((e.clusters, e.units), (1, 4));
    close(e.mean, 2.5);
    close(e.var, 5.0 / 3.0);
    assert_eq!(e.icc, None);
    // One unit per cluster (n = k): MSW has no degrees of freedom; the sample variance
    // of 1, 2, 4 is 7 / 3.
    let e = &pooled(&[&[1.0], &[2.0], &[4.0]])[0];
    assert_eq!((e.clusters, e.units), (3, 3));
    close(e.var, 7.0 / 3.0);
    assert_eq!(e.icc, None);
}

#[test]
fn fewer_than_two_units_give_no_estimate() {
    assert!(pooled(&[&[1.0]]).is_empty());
    assert!(pooled(&[]).is_empty());
    // Two units in two clusters do give one.
    assert_eq!(pooled(&[&[1.0], &[2.0]]).len(), 1);
    // A cluster with no complete unit is not counted.
    let e = &pooled(&[&[1.0, 3.0], &[], &[5.0]])[0];
    assert_eq!((e.clusters, e.units), (2, 3));
    close(e.var, 5.0);
}

#[test]
fn estimates_cover_each_stratum_and_the_pool() {
    let spec = one_hypothesis(&["fiction", "news"]);
    let mut rows = arm_a_rows("f1", &[1.0, 2.0]);
    rows.extend(arm_a_rows("f2", &[3.0, 4.0]));
    rows.extend(arm_a_rows("n1", &[10.0, 20.0]));
    // A unit without its reference row is incomplete and stays out.
    rows.push(ok("n1#2", "arm-a", 0, 99.0));
    // The selection cluster has values, and they stay out.
    rows.extend(arm_a_rows("s1", &[1000.0, 2000.0]));
    let clusters = [
        cluster("f1", "fiction", 2),
        cluster("f2", "fiction", 2),
        cluster("n1", "news", 3),
        // A selection cluster never counts.
        Cluster {
            role: Role::Selection,
            ..cluster("s1", "fiction", 2)
        },
    ];
    let p = paired(&spec, &rows);
    let all = estimates(&spec, &no_params(), &p, &clusters, None);
    let by: Vec<(&str, u64, u64)> = all
        .iter()
        .map(|e| (e.stratum.as_str(), e.clusters, e.units))
        .collect();
    assert_eq!(by, [("fiction", 2, 4), ("news", 1, 2), ("", 3, 6)]);
    close(all[1].mean, 15.0);
    close(all[1].var, 50.0);
    assert_eq!(all[1].icc, None);
    // The pool is the unweighted mean of the cluster means 1.5, 3.5 and 15.
    close(all[2].mean, 20.0 / 3.0);
    // Every estimate of one contrast shares the key and its hash.
    let terms = [("arm-a".to_string(), 1.0, json!({}))];
    let (key, hash) = contrast_key(&spec.outcomes[0], 1, &("base".into(), json!({})), &terms);
    assert!(all.iter().all(|e| e.key == key && e.key_hash == hash));
}

#[test]
fn the_key_carries_the_params_and_the_pick() {
    let mut v = ignorarium::spec::load("../../templates/numeric-ablation.toml".as_ref())
        .unwrap()
        .value;
    v["outcomes"][0]["paired"] = json!("difference");
    v["design"]["reference"] = json!("base");
    v["design"]["selection"] =
        json!({ "metric": "ppl-ratio", "conditions": ["arm-a", "arm-b"], "rule": "best-mean" });
    v["design"]["confirmation"] = json!({ "conditions": ["selected", "arm-a"] });
    v["question"]["hypotheses"][0]["contrast"] = json!({ "selected": 2.0, "arm-a": -1.0 });
    let spec = Spec::from_value(&v).unwrap();
    let mut params = no_params();
    params.insert("arm-b".into(), json!({ "bits": 8 }));
    params.insert("base".into(), json!({ "bits": 16 }));
    let pick = Selection {
        metric: "ppl-ratio".into(),
        rule: SelectionRule::BestMean,
        direction: Direction::Lower,
        candidates: vec![],
        selected: "arm-b".into(),
        units: vec![],
        complete_units: 0,
        incomplete_units: 0,
    };
    let mut rows = Vec::new();
    for i in 0..2 {
        let u = format!("c#{i}");
        rows.push(ok(&u, "base", 0, 10.0));
        rows.push(ok(&u, "arm-a", 0, 11.0 + f64::from(i)));
        // The row of the picked condition is recorded as `selected`.
        rows.push(ok(&u, "selected", 0, 15.0));
    }
    let p = paired(&spec, &rows);
    let clusters = [cluster("c", "", 2)];
    let e = estimates(&spec, &params, &p, &clusters, Some(&pick));
    // x = 2 * 5 - 1 = 9 and 2 * 5 - 2 = 8.
    assert_eq!(e.len(), 1);
    close(e[0].mean, 8.5);
    let terms = [
        ("arm-a".to_string(), -1.0, json!({})),
        ("arm-b".to_string(), 2.0, json!({ "bits": 8 })),
    ];
    let want = contrast_key(
        &spec.outcomes[0],
        1,
        &("base".into(), json!({ "bits": 16 })),
        &terms,
    );
    assert_eq!((e[0].key.clone(), e[0].key_hash.clone()), want);
    // Without the selection record a contrast on `selected` gives no estimate.
    assert!(estimates(&spec, &params, &p, &clusters, None).is_empty());
}

#[test]
fn a_repeated_stratum_gives_one_estimate() {
    let spec = one_hypothesis(&["fiction", "fiction"]);
    let mut rows = arm_a_rows("f1", &[1.0, 2.0]);
    rows.extend(arm_a_rows("f2", &[3.0, 4.0]));
    let clusters = [cluster("f1", "fiction", 2), cluster("f2", "fiction", 2)];
    let p = paired(&spec, &rows);
    let all = estimates(&spec, &no_params(), &p, &clusters, None);
    let strata: Vec<&str> = all.iter().map(|e| e.stratum.as_str()).collect();
    assert_eq!(strata, ["fiction", ""]);
}

#[test]
fn the_key_carries_the_repeats() {
    let mut v = ignorarium::spec::load("../../templates/numeric-ablation.toml".as_ref())
        .unwrap()
        .value;
    v["outcomes"][0]["paired"] = json!("difference");
    v["design"]["reference"] = json!("base");
    v["design"]["repeats"] = json!(3);
    let spec = Spec::from_value(&v).unwrap();
    let rows = arm_a_rows("c", &[1.0, 2.0]);
    let p = paired(&spec, &rows);
    let e = estimates(&spec, &no_params(), &p, &[cluster("c", "", 2)], None);
    let terms = [("arm-a".to_string(), 1.0, json!({}))];
    let want = contrast_key(&spec.outcomes[0], 3, &("base".into(), json!({})), &terms);
    assert_eq!((e[0].key.clone(), e[0].key_hash.clone()), want);
    // An estimate counts a unit that pairs in any repeat.
    assert_eq!(e[0].units, 2);
}

#[test]
fn clusters_that_hold_the_same_data_count_once() {
    let h = |a: &str, b: &str| Some((a.to_string(), b.to_string()));
    let hashes = vec![
        ("a".to_string(), h("x1", "n1")),
        // Same exact hash as `a`.
        ("b".to_string(), h("x1", "n9")),
        // Same normalized hash as `a`.
        ("c".to_string(), h("x2", "n1")),
        ("d".to_string(), h("x3", "n3")),
        // An unread input stands alone, twice.
        ("e".to_string(), None),
        ("f".to_string(), None),
        // An empty normalized hash matches nothing.
        ("g".to_string(), h("x4", "")),
        ("i".to_string(), h("x5", "")),
    ];
    let all: Vec<Cluster> = ["a", "b", "c", "d", "e", "f", "g", "i"]
        .iter()
        .map(|p| cluster(p, "", 1))
        .collect();
    let kept: Vec<String> = distinct_clusters(all.clone(), &hashes)
        .into_iter()
        .map(|c| c.path)
        .collect();
    assert_eq!(kept, ["a", "d", "e", "f", "g", "i"]);
    // Only a cluster of the same role is the same cluster.
    let mut mixed = all[..2].to_vec();
    mixed[1].role = Role::Selection;
    assert_eq!(distinct_clusters(mixed, &hashes).len(), 2);
}

// ---- verdicts ----

const HASH: [u8; 32] = [7; 32];
/// The level of the first slot of a stream.
const LEVEL: f64 = 0.005;

/// The template's one hypothesis, difference-paired against `base`, in a study of `mode`: it
/// holds `strata` to `claim` by `statistic`, with the whole level.
fn claimed(mode: &str, strata: &[&str], claim: Value, statistic: Value) -> Spec {
    Spec::from_value(&claimed_value(mode, strata, claim, statistic)).unwrap()
}

/// `claimed`, as the spec's value.
fn claimed_value(mode: &str, strata: &[&str], claim: Value, statistic: Value) -> Value {
    let mut v = ignorarium::spec::load("../../templates/numeric-ablation.toml".as_ref())
        .unwrap()
        .value;
    v["study"]["mode"] = json!(mode);
    v["outcomes"][0]["paired"] = json!("difference");
    v["design"]["reference"] = json!("base");
    let h = &mut v["question"]["hypotheses"][0];
    h["strata"] = json!(strata);
    h["claim"] = claim;
    h["statistic"] = statistic;
    h["alpha_share"] = json!(1.0);
    v
}

/// A claim below 0 on the mean, in the range [-1, 1].
fn below_zero(mode: &str, strata: &[&str]) -> Spec {
    Spec::from_value(&below_zero_value(mode, strata)).unwrap()
}

fn below_zero_value(mode: &str, strata: &[&str]) -> Value {
    claimed_value(
        mode,
        strata,
        json!({ "below": 0.0 }),
        json!({ "mean": [-1.0, 1.0] }),
    )
}

/// A confirmation cluster: its path, its stratum, and the paired value of each unit.
type Group = (String, String, Vec<f64>);

fn group(path: &str, stratum: &str, xs: &[f64]) -> Group {
    (path.into(), stratum.into(), xs.to_vec())
}

/// `n` clusters `<prefix><i>` in `stratum`, each of one unit at `x`.
fn same(prefix: &str, stratum: &str, n: usize, x: f64) -> Vec<Group> {
    (0..n)
        .map(|i| group(&format!("{prefix}{i}"), stratum, &[x]))
        .collect()
}

/// The paired values and the clusters of `groups`, every unit picked.
fn data(spec: &Spec, groups: &[Group]) -> (Paired, Vec<Cluster>) {
    let mut rows = Vec::new();
    let mut clusters = Vec::new();
    for (path, stratum, xs) in groups {
        rows.extend(arm_a_rows(path, xs));
        clusters.push(cluster(path, stratum, xs.len() as u32));
    }
    (paired(spec, &rows), clusters)
}

/// The verdicts, and the study's e-value merged from them.
fn decided(
    spec: &Spec,
    p: &Paired,
    clusters: &[Cluster],
    level: Option<f64>,
) -> (Vec<Verdict>, Option<f64>) {
    let vs = verdicts(spec, p, clusters, &HASH, level);
    let e = study_e_value(spec, &vs, level);
    (vs, e)
}

/// The order the test of hypothesis `id` takes the `n` clusters of `stratum` in.
fn seeded_order(id: &str, stratum: &str, n: usize) -> Vec<usize> {
    let path = format!("test/{id}/{stratum}");
    order(n, &mut Rng::from_seed(seed(&HASH, &path)))
}

fn bound(stratum: &str, side: &str, margin: f64, clusters: u64, value: f64) -> BoundResult {
    BoundResult {
        stratum: stratum.into(),
        side: side.into(),
        margin,
        clusters,
        value: Some(value),
    }
}

const MEAN: Statistic = Statistic::Mean { lo: -1.0, hi: 1.0 };
const BELOW_ZERO: Bound = Bound {
    tail: Tail::Lower,
    margin: 0.0,
};

/// The e-value of `n` clusters at -1 below 0 on [-1, 1]: each scores 1, and the n - 1 bets of
/// 1/2 give 1.5^(n - 1).
fn strong(n: usize) -> f64 {
    let e = e_value(&MEAN, &BELOW_ZERO, &vec![-1.0; n]).unwrap();
    assert!((e / 1.5f64.powi(n as i32 - 1) - 1.0).abs() < 1e-12, "{e}");
    e
}

#[test]
fn a_confirmatory_claim_tests_each_bound_on_one_observation_per_cluster_in_a_seeded_order() {
    let spec = claimed(
        "confirmatory",
        &[],
        json!({ "within": [-0.5, 0.5] }),
        json!({ "mean": [-1.0, 1.0] }),
    );
    // Cluster means 0.25, 0 (the 4 is clipped to 1 first), -0.25, 0.25 and 0.125.
    let groups = [
        group("c0", "", &[0.0, 0.5]),
        group("c1", "", &[4.0, -1.0]),
        group("c2", "", &[-0.25]),
        group("c3", "", &[0.25, 0.25, 0.25]),
        group("c4", "", &[0.125]),
    ];
    let (p, clusters) = data(&spec, &groups);
    let means = [0.25, 0.0, -0.25, 0.25, 0.125];
    let obs: Vec<f64> = (seeded_order("H1", "", 5).into_iter())
        .map(|i| means[i])
        .collect();
    let e = |tail, margin, obs: &[f64]| e_value(&MEAN, &Bound { tail, margin }, obs).unwrap();
    let (above, below) = (e(Tail::Upper, -0.5, &obs), e(Tail::Lower, 0.5, &obs));
    // The order matters: the clusters in spec order give another e-value.
    assert_ne!(above, e(Tail::Upper, -0.5, &means));
    // The means lie nearer the upper margin, so the hypothesis takes the lower bound's e-value.
    assert!(1.01 < below && below < above, "{below} {above}");
    // It passes at a level whose threshold 1 / level the e-value reaches, and fails just above.
    for (level, verdict) in [(1.01 / below, "pass"), (0.99 / below, "fail")] {
        let (vs, study) = decided(&spec, &p, &clusters, Some(level));
        let want = Verdict {
            hypothesis: "H1".into(),
            verdict: verdict.into(),
            basis: "test".into(),
            level: Some(level),
            e_value: Some(below),
            bounds: vec![
                bound("", "above", -0.5, 5, above),
                bound("", "below", 0.5, 5, below),
            ],
            incomplete_units: 0,
            reason: None,
        };
        assert_eq!(vs, [want], "{level}");
        // One hypothesis with the whole level: the study's e-value is its own.
        assert_eq!(study, Some(below), "{level}");
    }
}

#[test]
fn every_stratum_must_hold() {
    let spec = below_zero("confirmatory", &["code", "article"]);
    let mut groups = same("c", "code", 15, -1.0);
    groups.extend(same("a", "article", 15, 1.0));
    let (p, clusters) = data(&spec, &groups);
    let (vs, study) = decided(&spec, &p, &clusters, Some(LEVEL));
    // The code stratum alone passes; in the article stratum every cluster lies above 0, and
    // the test never bets.
    assert!(strong(15) >= 1.0 / LEVEL);
    let v = &vs[0];
    assert_eq!(
        v.bounds,
        [
            bound("code", "below", 0.0, 15, strong(15)),
            bound("article", "below", 0.0, 15, 1.0),
        ]
    );
    assert_eq!(
        (v.verdict.as_str(), v.e_value, study),
        ("fail", Some(1.0), Some(1.0))
    );
    // With the article clusters below 0 too, it passes.
    let groups = [same("c", "code", 15, -1.0), same("a", "article", 15, -1.0)].concat();
    let (p, clusters) = data(&spec, &groups);
    let (vs, _) = decided(&spec, &p, &clusters, Some(LEVEL));
    assert_eq!(
        (vs[0].verdict.as_str(), vs[0].e_value),
        ("pass", Some(strong(15)))
    );
}

#[test]
fn an_out_of_range_cluster_mean_is_clipped_into_the_claim() {
    // Each unit is clipped to the range [-0.1, 0.1] before the mean: clusters of three units
    // at -5 give -0.1, the end of the range, and score as -0.1 does. The test sees no value
    // out of range, so no cluster gives e-value 0.
    let spec = claimed(
        "confirmatory",
        &[],
        json!({ "below": 0.0 }),
        json!({ "mean": [-0.1, 0.1] }),
    );
    let groups: Vec<Group> = (0..15)
        .map(|i| group(&format!("c{i}"), "", &[-5.0; 3]))
        .collect();
    let (p, clusters) = data(&spec, &groups);
    let (vs, _) = decided(&spec, &p, &clusters, Some(LEVEL));
    let stat = Statistic::Mean { lo: -0.1, hi: 0.1 };
    let want = e_value(&stat, &BELOW_ZERO, &[-0.1; 15]).unwrap();
    assert!(want >= 1.0 / LEVEL);
    assert_eq!(
        (vs[0].verdict.as_str(), vs[0].e_value),
        ("pass", Some(want))
    );
}

#[test]
fn only_an_incomplete_unit_in_scope_fails_the_claim() {
    let spec = below_zero("confirmatory", &["code"]);
    let mut groups = same("c", "code", 15, -1.0);
    groups.push(group("a", "article", &[-1.0]));
    groups.push(group("s", "code", &[-1.0]));
    let (p, mut clusters) = data(&spec, &groups);
    clusters[16].role = Role::Selection;
    // Unit #1 of the article cluster and of the selection cluster has no paired value; neither
    // is in the scope of a claim on code.
    clusters[15].picked.push(1);
    clusters[16].picked.push(1);
    let judge =
        |spec: &Spec, clusters: &[Cluster]| decided(spec, &p, clusters, Some(LEVEL)).0.remove(0);
    let v = judge(&spec, &clusters);
    assert_eq!((v.verdict.as_str(), v.e_value), ("pass", Some(strong(15))));
    assert_eq!((v.incomplete_units, v.reason), (0, None));
    // A pooled claim holds every confirmation cluster, the article one included.
    let pooled = judge(&below_zero("confirmatory", &[]), &clusters);
    assert_eq!((pooled.incomplete_units, pooled.e_value), (1, Some(0.0)));
    // A code unit with no paired value is in scope: the claim fails closed, and says why.
    clusters[0].picked.push(1);
    let v = judge(&spec, &clusters);
    assert_eq!(
        (v.verdict.as_str(), v.e_value, v.incomplete_units),
        ("fail", Some(0.0), 1)
    );
    assert_eq!(v.reason.as_deref(), Some("incomplete units in scope: 1"));
    // The bound reports the test on the complete units, which alone would pass.
    assert_eq!(v.bounds, [bound("code", "below", 0.0, 15, strong(15))]);

    // With two repeats, a unit is complete only when each term pairs in both. Rows of c0#0
    // in `all`: repeat 0 reference and arm-a (0, 1), repeat 1 reference and arm-a (2, 3).
    let mut twice = below_zero("confirmatory", &[]);
    twice.design.repeats = 2;
    let clusters: Vec<Cluster> = (0..15).map(|i| cluster(&format!("c{i}"), "", 1)).collect();
    let all: Vec<Record> = (0..15)
        .flat_map(|i| {
            let u = format!("c{i}#0");
            (0..2).flat_map(move |r| [ok(&u, "base", r, 10.0), ok(&u, "arm-a", r, 9.0)])
        })
        .collect();
    let judge = |rows: &[Record]| decided(&twice, &paired(&twice, rows), &clusters, Some(LEVEL));
    let (vs, _) = judge(&all);
    assert_eq!(
        (vs[0].verdict.as_str(), vs[0].incomplete_units),
        ("pass", 0)
    );
    // A void arm-a row, or a void reference row, in one repeat: the other repeat still pairs,
    // and the unit is still incomplete.
    for gone in [3, 2] {
        let mut rows = all.clone();
        rows.remove(gone);
        let (vs, study) = judge(&rows);
        let v = &vs[0];
        assert_eq!(
            (v.verdict.as_str(), v.e_value, v.incomplete_units, study),
            ("fail", Some(0.0), 1, Some(0.0)),
            "row {gone}"
        );
        assert_eq!(v.reason.as_deref(), Some("incomplete units in scope: 1"));
    }
    // A contrast of two terms: one short in one repeat is enough, the other being whole.
    let mut two = twice.clone();
    two.question.hypotheses[0].contrast = [("arm-a".into(), 1.0), ("arm-b".into(), -1.0)].into();
    let arm_b = (0..15).flat_map(|i| (0..2).map(move |r| ok(&format!("c{i}#0"), "arm-b", r, 10.0)));
    let mut rows: Vec<Record> = all.iter().cloned().chain(arm_b).collect();
    let judge = |rows: &[Record]| decided(&two, &paired(&two, rows), &clusters, Some(LEVEL));
    assert_eq!(judge(&rows).0[0].incomplete_units, 0);
    // The last row of c0#0's: arm-b, repeat 1.
    let at = rows.len() - 29;
    assert_eq!(
        (
            rows[at].body["unit"].clone(),
            rows[at].body["repeat"].clone()
        ),
        (json!("c0#0"), json!(1))
    );
    rows.remove(at);
    let (vs, _) = judge(&rows);
    assert_eq!((vs[0].e_value, vs[0].incomplete_units), (Some(0.0), 1));
}

#[test]
fn an_exploratory_quantile_claim_passes_on_its_point_estimate() {
    // "At least 90 % of the 48 matrices at or below 0": one cluster of 48 units, and 2 more
    // units with no paired value. The point estimate counts the complete units only.
    let spec = claimed(
        "exploratory",
        &[],
        json!({ "below": 0.0 }),
        json!({ "quantile": 0.9 }),
    );
    let judge = |at_or_below: usize| {
        let xs: Vec<f64> = (0..48)
            .map(|i| if i < at_or_below { 0.0 } else { 1.0 })
            .collect();
        let (p, mut clusters) = data(&spec, &[group("m", "", &xs)]);
        clusters[0].picked.extend([48, 49]);
        decided(&spec, &p, &clusters, Some(LEVEL))
    };
    let (vs, study) = judge(44);
    let want = Verdict {
        hypothesis: "H1".into(),
        verdict: "pass".into(),
        basis: "estimate".into(),
        level: None,
        e_value: None,
        bounds: vec![bound("", "below", 0.0, 1, 44.0 / 48.0)],
        incomplete_units: 2,
        reason: None,
    };
    assert_eq!(vs, [want]);
    // No test, no e-value for the stream.
    assert_eq!(study, None);
    // 43 of 48 is under 0.9; so would 44 of 50 be, had the incomplete units counted.
    let (vs, _) = judge(43);
    assert_eq!(
        (vs[0].verdict.as_str(), vs[0].bounds[0].value),
        ("fail", Some(43.0 / 48.0))
    );
}

#[test]
fn an_exploratory_claim_leaves_out_a_unit_short_of_a_repeat() {
    // Two repeats, one cluster of 10 units: 9 pair to -1 in both repeats; m#9 pairs to +1 in
    // repeat 0, and its arm-a row of repeat 1 is void. The claim, below -0.9 on the mean, sees
    // the 9 complete units (mean -1) and passes; with m#9 the mean would be -0.8.
    let v = claimed_value(
        "exploratory",
        &[],
        json!({ "below": -0.9 }),
        json!({ "mean": [-1.0, 1.0] }),
    );
    let mut spec = Spec::from_value(&v).unwrap();
    spec.design.repeats = 2;
    let mut rows = Vec::new();
    for i in 0..10 {
        let u = format!("m#{i}");
        let x = if i == 9 { 11.0 } else { 9.0 };
        rows.extend([ok(&u, "base", 0, 10.0), ok(&u, "arm-a", 0, x)]);
        rows.push(ok(&u, "base", 1, 10.0));
        let exit = if i == 9 { "payload:1" } else { "ok" };
        rows.push(row(&u, "arm-a", 1, exit, Some(x)));
    }
    let (vs, study) = decided(&spec, &paired(&spec, &rows), &[cluster("m", "", 10)], None);
    let v = &vs[0];
    assert_eq!(
        (v.verdict.as_str(), v.incomplete_units, study),
        ("pass", 1, None)
    );
    assert_eq!(v.bounds, [bound("", "below", -0.9, 1, -1.0)]);
}

#[test]
fn a_descriptive_hypothesis_gets_verdict_none() {
    let spec = one_hypothesis(&[]);
    let (p, mut clusters) = data(&spec, &[group("c", "", &[1.0, 2.0])]);
    clusters[0].picked.push(2);
    let (vs, study) = decided(&spec, &p, &clusters, None);
    assert_eq!(study, None);
    let body = serde_json::to_value(&vs).unwrap();
    assert_eq!(
        body,
        json!([{
            "hypothesis": "H1", "verdict": "none", "basis": "none", "level": null,
            "e_value": null, "bounds": [], "incomplete_units": 1, "reason": null
        }])
    );
    // The record reads back as itself; one without a field, or with another, does not.
    let record = |body: Value| Record {
        kind: "verdict".into(),
        body,
        ..ok("u", "c", 0, 1.0)
    };
    assert_eq!(Verdict::of(&record(body[0].clone())), Some(vs[0].clone()));
    let mut short = body[0].clone();
    short.as_object_mut().unwrap().remove("e_value");
    assert_eq!(Verdict::of(&record(short)), None);
    let mut long = body[0].clone();
    long["extra"] = json!(1);
    assert_eq!(Verdict::of(&record(long)), None);
    // A confirmatory study with no claim to test still closes its slot: with e-value 0.
    let mut confirmatory = spec.clone();
    confirmatory.study.mode = ignorarium::spec::model::Mode::Confirmatory;
    let (vs, study) = decided(&confirmatory, &p, &clusters, Some(LEVEL));
    assert_eq!((vs[0].verdict.as_str(), study), ("none", Some(0.0)));
}

#[test]
fn the_level_splits_by_alpha_share() {
    // H1 holds code with share 0.75, H2 describes, and H3 holds article with share 0.25.
    let mut v = ignorarium::spec::load("../../templates/numeric-ablation.toml".as_ref())
        .unwrap()
        .value;
    v["study"]["mode"] = json!("confirmatory");
    v["outcomes"][0]["paired"] = json!("difference");
    v["design"]["reference"] = json!("base");
    let mut h1 = v["question"]["hypotheses"][0].clone();
    let mut h2 = h1.clone();
    h2["id"] = json!("H2");
    h1["strata"] = json!(["code"]);
    h1["claim"] = json!({ "below": 0.0 });
    h1["statistic"] = json!({ "mean": [-1.0, 1.0] });
    h1["alpha_share"] = json!(0.75);
    let mut h3 = h1.clone();
    h3["id"] = json!("H3");
    h3["strata"] = json!(["article"]);
    h3["alpha_share"] = json!(0.25);
    v["question"]["hypotheses"] = json!([h1, h2, h3]);
    let spec = Spec::from_value(&v).unwrap();
    let groups = [same("c", "code", 15, -1.0), same("a", "article", 16, -1.0)].concat();
    let (p, clusters) = data(&spec, &groups);
    let (vs, study) = decided(&spec, &p, &clusters, Some(LEVEL));
    let (e1, e3) = (strong(15), strong(16));
    // H1 needs 1 / 0.00375 = 267 and has 292; H3 needs 1 / 0.00125 = 800 and has 438, which
    // the whole level (200) would pass.
    let got: Vec<(&str, &str, Option<f64>, Option<f64>)> = (vs.iter())
        .map(|v| {
            (
                v.hypothesis.as_str(),
                v.verdict.as_str(),
                v.level,
                v.e_value,
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("H1", "pass", Some(LEVEL * 0.75), Some(e1)),
            ("H2", "none", None, None),
            ("H3", "fail", Some(LEVEL * 0.25), Some(e3)),
        ]
    );
    assert!(e3 >= 1.0 / LEVEL);
    // The study's e-value is the mean of the claims' e-values, weighted by their shares.
    let mean = 0.75 * e1 + 0.25 * e3;
    assert!((study.unwrap() - mean).abs() < 1e-9, "{study:?} {mean}");
    // A claim without a share leaves the level unsplit: every claim fails, and the study
    // closes with e-value 0.
    v["question"]["hypotheses"][2]
        .as_object_mut()
        .unwrap()
        .remove("alpha_share");
    let spec = Spec::from_value(&v).unwrap();
    let (vs, study) = decided(&spec, &p, &clusters, Some(LEVEL));
    let why = "the study's level does not split over its claims: invalid parameter: weights";
    for i in [0, 2] {
        assert_eq!(
            (vs[i].verdict.as_str(), vs[i].reason.as_deref()),
            ("fail", Some(why))
        );
    }
    assert_eq!((vs[1].reason.as_deref(), study), (None, Some(0.0)));
}

#[test]
fn a_claim_the_test_cannot_take_fails_closed() {
    // A margin at an end of the mean range, and a claim with no statistic: the rules refuse
    // both, and a run that meets one fails the claim, with e-value 0 in a test.
    let mut cases = Vec::new();
    for (mode, tested) in [("confirmatory", true), ("exploratory", false)] {
        let claim = json!({ "below": 1.0 });
        let edge = claimed(mode, &[], claim.clone(), json!({ "mean": [-1.0, 1.0] }));
        let mut bare = edge.clone();
        bare.question.hypotheses[0].statistic = None;
        cases.push((edge, "invalid parameter: bounds", tested));
        cases.push((bare, "invalid parameter: statistic", tested));
    }
    for (spec, why, tested) in cases {
        let (p, clusters) = data(&spec, &same("c", "", 15, -1.0));
        let (vs, _) = decided(&spec, &p, &clusters, Some(LEVEL));
        let v = &vs[0];
        assert_eq!(v.verdict, "fail", "{why}");
        assert_eq!(
            v.reason,
            Some(format!("the claim cannot be tested: {why}")),
            "{why}"
        );
        assert_eq!(v.e_value, tested.then_some(0.0), "{why}");
        assert!(v.bounds.is_empty(), "{why}");
    }
}

#[test]
fn an_e_value_too_large_for_a_float_is_kept_finite() {
    // 1,800 clusters at -1: 1.5^1799 overflows a float. Records and the stream hold finite
    // numbers, and the largest float is a smaller e-value, so still one.
    let spec = below_zero("confirmatory", &[]);
    let (p, clusters) = data(&spec, &same("c", "", 1800, -1.0));
    assert_eq!(
        e_value(&MEAN, &BELOW_ZERO, &[-1.0; 1800]).unwrap(),
        f64::INFINITY
    );
    let (vs, study) = decided(&spec, &p, &clusters, Some(LEVEL));
    assert_eq!(
        (vs[0].verdict.as_str(), vs[0].e_value, study),
        ("pass", Some(f64::MAX), Some(f64::MAX))
    );
    assert_eq!(vs[0].bounds[0].value, Some(f64::MAX));
    assert_eq!(
        serde_json::to_value(&vs[0]).unwrap()["e_value"],
        json!(f64::MAX)
    );

    // Five claims at the largest float, with shares whose float sum is 1: their weighted mean
    // overflows, so the study's e-value is clamped once more, and the stream takes it.
    let shares = [0.1, 0.12, 0.48, 0.25, 0.05];
    assert_eq!(shares.iter().sum::<f64>(), 1.0);
    let merged = tree::decide(LEVEL, &shares, &[Some(f64::MAX); 5]).unwrap();
    assert_eq!(merged.e_value, f64::INFINITY);
    let mut v = below_zero_value("confirmatory", &[]);
    let h = v["question"]["hypotheses"][0].clone();
    let five: Vec<Value> = (shares.iter().enumerate())
        .map(|(i, s)| {
            let mut h = h.clone();
            h["id"] = json!(format!("H{i}"));
            h["alpha_share"] = json!(s);
            h
        })
        .collect();
    v["question"]["hypotheses"] = json!(five);
    let five = Spec::from_value(&v).unwrap();
    let (vs, study) = decided(&five, &p, &clusters, Some(LEVEL));
    assert!(
        vs.iter()
            .all(|v| v.verdict == "pass" && v.e_value == Some(f64::MAX))
    );
    assert_eq!(study, Some(f64::MAX));
    let dir = tempfile::TempDir::new().unwrap();
    let home = Home::open_at(dir.path()).unwrap();
    let key = keys::load_or_create(&home).unwrap();
    let log = Log::of(&home, LogName::Plan);
    let slot = log.open(&key, "s", 0.0).unwrap().slot;
    // A discovery at the slot's level.
    assert!(log.close(&key, "s", slot, f64::MAX).unwrap());
}
