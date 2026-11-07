//! The blinded re-size, decided from a first batch's row records alone.
mod common;
use evidence::claim::{Bound, Statistic};
use evidence::power::{Plan, resize_count};
use ignorarium::canon::sha256_hex;
use ignorarium::derived::{self, CONTINUE, Resize, ResizeStratum, STOP};
use ignorarium::estimate::apply_defaults;
use ignorarium::records::Record;
use ignorarium::resize::{decide, or_stop};
use ignorarium::schedule::reserve_order;
use ignorarium::seed::seed;
use ignorarium::spec::model::{Role, Spec};
use ignorarium::units::{self, Cluster};
use serde_json::{Value, json};
use std::path::Path;

const HASH: [u8; 32] = [7; 32];
/// The level the register record gives the study.
const LEVEL: f64 = 0.005;
const SHORT: &str = "the reserve cannot reach the power the claim needs";

/// The resize fixture with `first` confirmation and `reserve` reserve clusters of one unit in
/// each of `fiction` and `news`, the claim's statistic a mean in [-2, 2] at power_at -1, and
/// its outcome a plain difference. A reserve input's pin is the SHA-256 of its path, so the
/// pins' order is not the paths'.
fn study(first: usize, reserve: usize) -> Value {
    let raw = ignorarium::spec::load(Path::new("tests/fixtures/specs/resize.toml")).unwrap();
    let mut v = raw.value;
    apply_defaults(&mut v);
    let template = v["inputs"][0].clone();
    let mut inputs = vec![];
    for (role, n) in [("confirmation", first), ("reserve", reserve)] {
        for s in ["fiction", "news"] {
            for i in 0..n {
                let path = format!("data/{s}-{role}-{i:02}.txt");
                let mut input = template.clone();
                input["sha256"] = json!(sha256_hex(path.as_bytes()));
                input["path"] = json!(path);
                input["role"] = json!(role);
                input["stratum"] = json!(s);
                input["units"] = json!(1);
                inputs.push(input);
            }
        }
    }
    v["inputs"] = json!(inputs);
    v["question"]["hypotheses"][0]["statistic"] = json!({ "mean": [-2.0, 2.0] });
    v["outcomes"][0]["paired"] = json!("difference");
    v
}

fn spec_of(v: &Value) -> Spec {
    Spec::from_value(v).unwrap()
}

fn clusters_of(spec: &Spec) -> Vec<Cluster> {
    units::clusters(spec, &HASH)
}

/// The register record, which gives the study `LEVEL`, then two row records per unit of each
/// cluster of `role`: the reference at 100 and `arm-a` at 100 + `value(cluster)`.
fn records(spec: &Spec, role: Role, value: impl Fn(&Cluster) -> f64) -> Vec<Record> {
    let register = derived::Register {
        spec_hash: format!("sha256:{}", hex::encode(HASH)),
        leaf: 0,
        log: "planlog".into(),
        defaults: json!([]),
        root_id: "12".repeat(32),
        payload: "cd".repeat(32),
        inputs: vec![],
        resolved: vec![],
        deviations: vec![],
        carries: None,
        family: None,
        alpha: Some(derived::Alpha {
            log: "planlog".into(),
            slot: 0,
            level: LEVEL,
        }),
        timestamp: None,
    };
    let mut bodies = vec![("register", serde_json::to_value(register).unwrap())];
    bodies.extend(rows(spec, role, value));
    common::records_for(&spec.study.id, &bodies)
}

/// The row record bodies of `records` alone.
fn rows(spec: &Spec, role: Role, value: impl Fn(&Cluster) -> f64) -> Vec<(&'static str, Value)> {
    let stage = if role == Role::Reserve {
        "reserve"
    } else {
        "confirmation"
    };
    let mut out = vec![];
    for c in clusters_of(spec).iter().filter(|c| c.role == role) {
        for &i in &c.picked {
            for (condition, ppl) in [("exact", 100.0), ("arm-a", 100.0 + value(c))] {
                let body = json!({
                    "stage": stage, "unit": units::key(&c.path, i), "repeat": 0,
                    "condition": condition, "exit": "ok", "values": { "ppl": ppl },
                });
                out.push(("row", body));
            }
        }
    }
    out
}

/// Every cluster's observation by stratum: fiction's, then news'.
fn by_stratum(fiction: f64, news: f64) -> impl Fn(&Cluster) -> f64 {
    move |c| {
        if c.stratum == "fiction" {
            fiction
        } else {
            news
        }
    }
}

/// The re-size count of a group whose observations have blinded sd `s`: the fixture's claim at
/// `LEVEL` x `share`, sized at `at` for `target`, over `cap` clusters of one unit.
fn count(s: f64, path: &str, cap: u32, target: f64, at: f64, share: f64) -> Option<u64> {
    count_under(&HASH, s, path, cap, target, at, share)
}

/// `count` under spec hash `hash`.
fn count_under(
    hash: &[u8; 32],
    s: f64,
    path: &str,
    cap: u32,
    target: f64,
    at: f64,
    share: f64,
) -> Option<u64> {
    let plan = Plan {
        stat: Statistic::Mean { lo: -2.0, hi: 2.0 },
        bounds: vec![Bound {
            tail: evidence::Tail::Lower,
            margin: 0.0,
        }],
        strata: vec![],
        level: LEVEL * share,
    };
    let sizes = vec![1; cap as usize];
    let n = resize_count(&plan, &sizes, s, at, target, 4000, seed(hash, path), cap);
    n.unwrap().map(u64::from)
}

/// The power each of the fixture's two strata is sized for.
fn each() -> f64 {
    0.8f64.powf(1.0 / 2.0)
}

fn group(stratum: &str, first: u64, reserve: u64, needed: Option<u64>) -> ResizeStratum {
    let appended = needed.map_or(0, |n| n.saturating_sub(first));
    ResizeStratum {
        stratum: stratum.into(),
        first,
        reserve,
        needed,
        appended,
    }
}

/// The paths of the reserve clusters of `stratum` (every one for `""`) in the order of their
/// stratum, then their pins, as the spec lists them: the order the re-size appends.
fn by_pin(spec: &Spec, stratum: &str) -> Vec<String> {
    let mut held: Vec<(&str, &str, &str)> = (spec.inputs.iter())
        .filter(|i| i.role == Role::Reserve && (stratum.is_empty() || i.stratum == stratum))
        .map(|i| (&i.stratum[..], &i.sha256[..], &i.path[..]))
        .collect();
    held.sort();
    held.into_iter().map(|(_, _, p)| p.to_string()).collect()
}

fn admitted(spec: &Spec, r: &Resize) -> Vec<String> {
    let clusters = clusters_of(spec);
    let order = reserve_order(spec, &clusters);
    let mut paths: Vec<String> = r.admitted(&order).into_iter().map(String::from).collect();
    // In the order the re-size appends them.
    let rank = |p: &String| order.iter().position(|c| c.path == *p).unwrap();
    paths.sort_by_key(rank);
    paths
}

#[test]
fn the_count_uses_the_uncentred_sd_of_the_first_batch() {
    let spec = spec_of(&study(30, 16));
    // Every observation is 0.5: the sd about their mean is 0, about 0 it is 0.5.
    let all = records(&spec, Role::Confirmation, |_| 0.5);
    let r = decide(&spec, &HASH, &all, &clusters_of(&spec)).unwrap();
    let at = |s: f64, g: &str| count(s, &format!("resize/H1/{g}"), 46, each(), -1.0, 1.0);
    for g in ["fiction", "news"] {
        assert_ne!(at(0.5, g), at(0.0, g), "{g}");
    }
    assert_eq!(
        r.strata,
        [
            group("fiction", 30, 16, at(0.5, "fiction")),
            group("news", 30, 16, at(0.5, "news")),
        ]
    );
    assert_eq!((r.decision.as_str(), r.reason), (CONTINUE, None));
}

#[test]
fn the_record_holds_counts_and_no_sd() {
    let spec = spec_of(&study(30, 16));
    let all = records(&spec, Role::Confirmation, by_stratum(-1.0, 0.75));
    let r = decide(&spec, &HASH, &all, &clusters_of(&spec)).unwrap();
    let v = serde_json::to_value(&r).unwrap();
    let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(keys(&v), ["decision", "reason", "strata"]);
    assert_eq!(v["reason"], Value::Null);
    for g in v["strata"].as_array().unwrap() {
        assert_eq!(
            keys(g),
            ["appended", "first", "needed", "reserve", "stratum"]
        );
        for k in ["appended", "first", "needed", "reserve"] {
            assert!(g[k].is_u64(), "{g}");
        }
    }
    // The record parses back as it was written.
    let record = common::record("resize-study", 9, "resize", v);
    assert_eq!(Resize::of(&record), Some(r));
}

#[test]
fn continue_appends_needed_minus_first_in_pin_order() {
    let spec = spec_of(&study(24, 16));
    // Fiction's spread is 1, news' 0.75: each stratum takes its own count.
    let all = records(&spec, Role::Confirmation, by_stratum(-1.0, 0.75));
    let r = decide(&spec, &HASH, &all, &clusters_of(&spec)).unwrap();
    let fiction = count(1.0, "resize/H1/fiction", 40, each(), -1.0, 1.0);
    let news = count(0.75, "resize/H1/news", 40, each(), -1.0, 1.0);
    assert_eq!((fiction, news), (Some(32), Some(26)));
    assert_eq!(
        r.strata,
        [
            group("fiction", 24, 16, fiction),
            group("news", 24, 16, news)
        ]
    );
    assert_eq!((r.strata[0].appended, r.strata[1].appended), (8, 2));
    assert_eq!(r.decision, CONTINUE);
    // The first 8 of fiction's reserve and the first 2 of news', by pin, not by path.
    let pinned = |s: &str, n: usize| by_pin(&spec, s)[..n].to_vec();
    let want = [pinned("fiction", 8), pinned("news", 2)].concat();
    assert_ne!(
        pinned("fiction", 8),
        (0..8)
            .map(|i| format!("data/fiction-reserve-{i:02}.txt"))
            .collect::<Vec<_>>()
    );
    assert_eq!(admitted(&spec, &r), want);
}

#[test]
fn each_stratum_counts_its_own_clusters() {
    // News keeps 20 first and 10 reserve clusters; fiction keeps 30 and 16.
    let mut v = study(30, 16);
    let dropped = |p: &str| {
        (20..30).any(|i| p == format!("data/news-confirmation-{i}.txt"))
            || (10..16).any(|i| p == format!("data/news-reserve-{i}.txt"))
    };
    let kept: Vec<Value> = (v["inputs"].as_array().unwrap().iter())
        .filter(|i| !dropped(i["path"].as_str().unwrap()))
        .cloned()
        .collect();
    v["inputs"] = json!(kept);
    let spec = spec_of(&v);
    let all = records(&spec, Role::Confirmation, by_stratum(-1.0, 0.5));
    let r = decide(&spec, &HASH, &all, &clusters_of(&spec)).unwrap();
    let fiction = count(1.0, "resize/H1/fiction", 46, each(), -1.0, 1.0);
    let news = count(0.5, "resize/H1/news", 30, each(), -1.0, 1.0);
    assert_eq!((fiction, news), (Some(32), Some(22)));
    assert_eq!(
        r.strata,
        [
            group("fiction", 30, 16, fiction),
            group("news", 20, 10, news)
        ]
    );
}

#[test]
fn stop_when_the_reserve_is_short() {
    let spec = spec_of(&study(30, 16));
    // News' spread of 2 needs more clusters than its 46: the study stops, and appends nothing.
    let all = records(&spec, Role::Confirmation, by_stratum(-1.0, -2.0));
    let r = decide(&spec, &HASH, &all, &clusters_of(&spec)).unwrap();
    assert_eq!(count(2.0, "resize/H1/news", 46, each(), -1.0, 1.0), None);
    let fiction = count(1.0, "resize/H1/fiction", 46, each(), -1.0, 1.0);
    assert_eq!(
        r,
        Resize {
            strata: vec![
                ResizeStratum {
                    appended: 0,
                    ..group("fiction", 30, 16, fiction)
                },
                group("news", 30, 16, None),
            ],
            decision: STOP.into(),
            reason: Some(SHORT.into()),
        }
    );
    assert_eq!(admitted(&spec, &r), Vec::<String>::new());
    // One cluster more in news' reserve than it needs is enough to continue.
    let spec = spec_of(&study(30, 16));
    let fits = records(&spec, Role::Confirmation, by_stratum(-1.0, -1.0));
    let r = decide(&spec, &HASH, &fits, &clusters_of(&spec)).unwrap();
    assert_eq!(r.decision, CONTINUE);
}

#[test]
fn a_stop_record_admits_nothing_whatever_it_counts() {
    let spec = spec_of(&study(30, 16));
    let r = Resize {
        strata: vec![group("fiction", 30, 16, Some(40))],
        decision: STOP.into(),
        reason: Some(SHORT.into()),
    };
    assert_eq!(admitted(&spec, &r), Vec::<String>::new());
    let go = Resize {
        decision: CONTINUE.into(),
        ..r
    };
    assert_eq!(
        admitted(&spec, &go),
        by_pin(&spec, "fiction")[..10].to_vec()
    );
}

#[test]
fn an_incomplete_first_batch_stops() {
    let spec = spec_of(&study(30, 16));
    let mut all = records(&spec, Role::Confirmation, |_| -1.0);
    let clusters = clusters_of(&spec);
    assert!(decide(&spec, &HASH, &all, &clusters).is_ok());
    // One unit's arm-a row failed: its value is missing.
    let at = all
        .iter()
        .position(|r| r.body["condition"] == "arm-a")
        .unwrap();
    all[at].body["exit"] = json!("payload:1");
    let why = "incomplete units in the first batch: 1";
    assert_eq!(decide(&spec, &HASH, &all, &clusters), Err(why.to_string()));
    let r = or_stop(&spec, &HASH, &all, &clusters);
    assert_eq!(
        r,
        Resize {
            strata: vec![group("fiction", 30, 16, None), group("news", 30, 16, None)],
            decision: STOP.into(),
            reason: Some(why.into()),
        }
    );
    // A missing row record counts the same.
    all.remove(at);
    assert_eq!(decide(&spec, &HASH, &all, &clusters), Err(why.to_string()));
    // A complete batch gives `or_stop` the decision itself.
    let whole = records(&spec, Role::Confirmation, |_| -1.0);
    let made = decide(&spec, &HASH, &whole, &clusters).unwrap();
    assert_eq!(or_stop(&spec, &HASH, &whole, &clusters), made);
}

#[test]
fn a_study_without_a_level_stops() {
    let spec = spec_of(&study(30, 16));
    let mut all = records(&spec, Role::Confirmation, |_| -1.0);
    all.remove(0);
    let why = "the study holds no level";
    assert_eq!(
        decide(&spec, &HASH, &all, &clusters_of(&spec)),
        Err(why.to_string())
    );
}

#[test]
fn the_decision_is_the_same_from_the_same_records() {
    let spec = spec_of(&study(30, 16));
    let clusters = clusters_of(&spec);
    let all = records(&spec, Role::Confirmation, by_stratum(-1.0, 0.75));
    let once = decide(&spec, &HASH, &all, &clusters).unwrap();
    assert_eq!(decide(&spec, &HASH, &all, &clusters).unwrap(), once);
    // Reserve rows, whatever they hold, and the records' order do not change it.
    let mut more = all.clone();
    let reserve = rows(&spec, Role::Reserve, |_| 1.75);
    more.extend(common::records_for(&spec.study.id, &reserve));
    more[1..].reverse();
    assert_eq!(decide(&spec, &HASH, &more, &clusters).unwrap(), once);
    // The spec hash seeds the count: the first other hash that moves fiction's count moves
    // the decision's to the same value.
    let fiction = |h: &[u8; 32]| count_under(h, 1.0, "resize/H1/fiction", 46, each(), -1.0, 1.0);
    let (hash, moved) = (1..=64u8)
        .map(|b| [b; 32])
        .find_map(|h| Some((h, fiction(&h))).filter(|(_, n)| *n != once.strata[0].needed))
        .expect("some spec hash moves the count");
    let other = decide(&spec, &hash, &all, &clusters).unwrap();
    assert_eq!(other.strata[0].needed, moved);
}

#[test]
fn a_pooled_claim_sizes_every_first_cluster_as_one_group() {
    let mut v = study(10, 16);
    v["question"]["hypotheses"][0]["strata"] = json!([]);
    let spec = spec_of(&v);
    let all = records(&spec, Role::Confirmation, |_| -1.25);
    let r = decide(&spec, &HASH, &all, &clusters_of(&spec)).unwrap();
    // One group of 20 first and 32 reserve clusters, sized for the whole power.
    let needed = count(1.25, "resize/H1/", 52, 0.8, -1.0, 1.0);
    assert_eq!(needed, Some(37));
    assert_eq!(r.strata, [group("", 20, 32, needed)]);
    assert_eq!(r.decision, CONTINUE);
    // 17 appended: every fiction reserve cluster by pin, then the first of news'.
    let want = [
        by_pin(&spec, "fiction"),
        by_pin(&spec, "news")[..1].to_vec(),
    ]
    .concat();
    assert_eq!(want, by_pin(&spec, "")[..17].to_vec());
    assert_eq!(admitted(&spec, &r), want);
}

#[test]
fn the_largest_count_over_the_claims_wins() {
    let mut v = study(30, 16);
    let mut h2 = v["question"]["hypotheses"][0].clone();
    h2["id"] = json!("H2");
    h2["power_at"] = json!(-1.5);
    v["question"]["hypotheses"][0]["alpha_share"] = json!(0.5);
    h2["alpha_share"] = json!(0.5);
    let h1 = v["question"]["hypotheses"][0].clone();
    let fiction =
        |id: &str, at: f64| count(1.0, &format!("resize/{id}/fiction"), 46, each(), at, 0.5);
    let news = |id: &str, at: f64| count(0.75, &format!("resize/{id}/news"), 46, each(), at, 0.5);
    let (h1_fiction, h2_fiction) = (fiction("H1", -1.0), fiction("H2", -1.5));
    assert!(h1_fiction > h2_fiction, "{h1_fiction:?} {h2_fiction:?}");
    let want = [
        group("fiction", 30, 16, h1_fiction),
        group("news", 30, 16, news("H1", -1.0).max(news("H2", -1.5))),
    ];
    // In either order of the hypotheses.
    for list in [json!([h1, h2]), json!([h2, h1])] {
        v["question"]["hypotheses"] = list;
        let spec = spec_of(&v);
        let all = records(&spec, Role::Confirmation, by_stratum(-1.0, 0.75));
        let r = decide(&spec, &HASH, &all, &clusters_of(&spec)).unwrap();
        assert_eq!(r.strata, want);
    }
}
