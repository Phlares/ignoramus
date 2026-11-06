//! Reading the rows back: each condition's value for a unit, paired with the reference
//! condition's value for the same unit and repeat.
use crate::canon::{sha256_hex, to_canonical};
use crate::derived::{
    ANALYSIS, Analysis, BoundResult, CONTINUE, Candidate, ESTIMATE, Estimate, Inputs, Instrument,
    Recheck, Register, Selection, VERDICT, Verdict, contrast_key,
};
use crate::planlog::store::{PlanLog, RELEASE};
use crate::prior::Seen;
use crate::records::{self, Record, RecordDraft};
use crate::resize;
use crate::runner::{InputHashes, RunError, RunPlan, own_label, stored_spec, typed};
use crate::schedule::{self, Row, SELECTION_STAGE};
use crate::seed::seed;
use crate::spec;
use crate::spec::model::{
    Claim, Direction, Hypothesis, Mode, Paired as Rule, Role, SELECTED, Spec,
};
use crate::stream::{Log, LogName};
use crate::units::{self, Cluster};
use evidence::rng::Rng;
use evidence::{Tail, claim, tree};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

/// Paired values by (unit, condition, outcome id), each with the number of repeats it is the
/// mean of.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Paired {
    by_key: BTreeMap<(String, String, String), (f64, u32)>,
}

impl Paired {
    /// The mean over the repeats that pair; `None` when no repeat does.
    pub fn get(&self, unit: &str, condition: &str, outcome: &str) -> Option<f64> {
        self.entry(unit, condition, outcome).map(|(mean, _)| mean)
    }

    /// The mean and the number of repeats that pair; `None` when no repeat does.
    fn entry(&self, unit: &str, condition: &str, outcome: &str) -> Option<(f64, u32)> {
        let key = (unit.to_string(), condition.to_string(), outcome.to_string());
        self.by_key.get(&key).copied()
    }

    /// The units with at least one paired value, in order.
    pub fn units(&self) -> impl Iterator<Item = &str> {
        let mut last: Option<&str> = None;
        self.by_key.keys().filter_map(move |(u, _, _)| {
            let fresh = last != Some(u.as_str());
            last = Some(u.as_str());
            fresh.then_some(u.as_str())
        })
    }
}

/// The row records of the schedule's own rows: a recheck copy repeats a row to compare its
/// output, and adds no value.
fn own_rows(records: &[Record]) -> impl Iterator<Item = &Record> {
    (records.iter()).filter(|r| r.kind == "row" && !is_recheck(r))
}

fn is_recheck(r: &Record) -> bool {
    r.body.get("recheck_of").is_some()
}

/// Pair every `ok` row record's values with the reference condition's row for the same
/// unit and repeat; recheck copies add none. `Difference` is condition minus reference;
/// `Percent` is 100 x (condition / reference - 1), and no pair when the reference is not above
/// zero; `LogRatio` is ln(condition / reference), and no pair when either side is not above
/// zero.
pub fn paired(spec: &Spec, rows: &[Record]) -> Paired {
    // (unit, repeat, condition) -> the row's values
    let mut cells: BTreeMap<(&str, u64, &str), &serde_json::Value> = BTreeMap::new();
    for r in own_rows(rows) {
        let b = &r.body;
        if b["exit"] != "ok" {
            continue;
        }
        if let (Some(u), Some(rep), Some(c), v) = (
            b["unit"].as_str(),
            b["repeat"].as_u64(),
            b["condition"].as_str(),
            &b["values"],
        ) {
            cells.insert((u, rep, c), v);
        }
    }
    let reference = spec.design.reference.as_str();
    let mut sums: BTreeMap<(String, String, String), (f64, u32)> = BTreeMap::new();
    for (&(unit, repeat, cond), values) in &cells {
        let Some(base) = cells.get(&(unit, repeat, reference)) else {
            continue;
        };
        for o in &spec.outcomes {
            let (Some(a), Some(b)) = (values[&o.measure].as_f64(), base[&o.measure].as_f64())
            else {
                continue;
            };
            let v = match o.paired {
                Rule::Difference => a - b,
                Rule::Percent if b > 0.0 => 100.0 * (a / b - 1.0),
                Rule::Percent => continue,
                // ln of a side at or below zero is not finite, which drops the pair below.
                Rule::LogRatio => a.ln() - b.ln(),
            };
            if !v.is_finite() {
                continue;
            }
            let e = sums
                .entry((unit.to_string(), cond.to_string(), o.id.clone()))
                .or_default();
            e.0 += v;
            e.1 += 1;
        }
    }
    Paired {
        by_key: sums
            .into_iter()
            .map(|(k, (sum, n))| (k, (sum / f64::from(n), n)))
            .filter(|(_, (v, _))| v.is_finite())
            .collect(),
    }
}

/// The selection stage's pick. A selection unit (a unit of a `selection` row record, recheck
/// copies aside) is complete when it has a paired metric value for every candidate. Each
/// candidate's mean is over the complete units; the best mean in the metric outcome's
/// direction wins, and a tie goes to the first listed. `Err` when the plan has no stage or
/// candidate, its metric is not an outcome, no unit is complete, or a mean is not finite.
pub fn select(spec: &Spec, rows: &[Record]) -> Result<Selection, String> {
    let stage = (spec.design.selection.as_ref()).ok_or("the plan has no selection stage")?;
    let outcome = (spec.outcomes.iter().find(|o| o.id == stage.metric))
        .ok_or("the selection metric is not an outcome of the plan")?;
    let units: BTreeSet<&str> = own_rows(rows)
        .filter(|r| r.body["stage"] == SELECTION_STAGE)
        .filter_map(|r| r.body["unit"].as_str())
        .collect();
    let pairs = paired(spec, rows);
    // (unit, its paired value per candidate) of each complete unit, sorted by unit.
    let complete: Vec<(&str, Vec<f64>)> = units
        .iter()
        .filter_map(|u| {
            let values = (stage.conditions.iter())
                .map(|c| pairs.get(u, c, &outcome.id))
                .collect::<Option<_>>()?;
            Some((*u, values))
        })
        .collect();
    if complete.is_empty() {
        return Err(format!(
            "no selection unit has a {} value for every candidate; incomplete units: {}",
            outcome.id,
            units.len()
        ));
    }
    let n = complete.len() as f64;
    let mut candidates = Vec::new();
    for (i, id) in stage.conditions.iter().enumerate() {
        let mean = complete.iter().map(|(_, v)| v[i]).sum::<f64>() / n;
        if !mean.is_finite() {
            return Err(format!(
                "the mean {} of candidate {id} is not finite",
                outcome.id
            ));
        }
        candidates.push(Candidate {
            id: id.clone(),
            mean,
        });
    }
    let better = |a: f64, b: f64| match outcome.direction {
        Direction::Lower => a < b,
        Direction::Higher => a > b,
    };
    let best = (candidates.iter())
        .reduce(|best, c| if better(c.mean, best.mean) { c } else { best })
        .ok_or("the selection stage lists no candidate")?;
    Ok(Selection {
        metric: outcome.id.clone(),
        rule: stage.rule,
        direction: outcome.direction,
        selected: best.id.clone(),
        units: complete.iter().map(|(u, _)| u.to_string()).collect(),
        complete_units: complete.len() as u64,
        incomplete_units: (units.len() - complete.len()) as u64,
        candidates,
    })
}

/// One estimate per hypothesis, per group: each stratum the hypothesis lists, then the pooled
/// group `""` (alone when it lists none). A group holds the confirmation clusters of its
/// stratum, then its reserve clusters in `clusters`. A unit's value is the contrast over its
/// paired values, and counts only when every term has one. A `selected` row is recorded under
/// `selected`, so its value is read there, while the key names the pick. `params` holds each
/// condition's params, by id. A hypothesis whose outcome is unknown, or whose contrast needs a
/// pick that `selection` does not give, has none, and neither has a group with fewer than 2
/// values.
pub fn estimates(
    spec: &Spec,
    params: &BTreeMap<String, Value>,
    paired: &Paired,
    clusters: &[Cluster],
    selection: Option<&Selection>,
) -> Vec<Estimate> {
    let params_of = |c: &str| params.get(c).cloned().unwrap_or_else(|| json!({}));
    let reference = (
        spec.design.reference.clone(),
        params_of(&spec.design.reference),
    );
    let mut out = Vec::new();
    for h in &spec.question.hypotheses {
        let Some(outcome) = spec.outcomes.iter().find(|o| o.id == h.outcome) else {
            continue;
        };
        let terms: Option<Vec<(String, f64, Value)>> = (h.contrast.iter())
            .map(|(c, w)| {
                let c = if c == SELECTED {
                    selection?.selected.as_str()
                } else {
                    c
                };
                Some((c.to_string(), *w, params_of(c)))
            })
            .collect();
        let Some(terms) = terms else { continue };
        let (key, key_hash) =
            contrast_key(outcome, u64::from(spec.design.repeats), &reference, &terms);
        let mut groups = claim_strata(h);
        if !groups.contains(&"") {
            groups.push("");
        }
        for stratum in groups {
            let samples = samples(clusters, h, stratum, paired);
            if let Some(f) = fit(&samples) {
                out.push(Estimate {
                    hypothesis: h.id.clone(),
                    outcome: outcome.id.clone(),
                    key: key.clone(),
                    key_hash: key_hash.clone(),
                    stratum: stratum.into(),
                    clusters: samples.len() as u64,
                    units: samples.iter().map(Vec::len).sum::<usize>() as u64,
                    mean: f.mean,
                    var: f.var,
                    icc: f.icc,
                });
            }
        }
    }
    out
}

/// The values `estimates` fits for `h` in group `stratum`: per cluster of `clusters`, the
/// confirmation clusters then the reserve ones, the contrast's value of each unit that pairs
/// in any repeat.
fn samples(clusters: &[Cluster], h: &Hypothesis, stratum: &str, paired: &Paired) -> Vec<Vec<f64>> {
    let of = |role| group(clusters, role, stratum, &h.outcome, &h.contrast, paired, 1).0;
    [of(Role::Confirmation), of(Role::Reserve)].concat()
}

/// A finished study read again from what the box keeps of it: the plan text it stored at
/// registration and its signed records. It gives the unit values its estimates were fitted
/// from, so a later plan can fit them as its own test reads them.
pub struct Source {
    spec: Spec,
    pairs: Paired,
    clusters: Vec<Cluster>,
}

/// What `Source::refit` gives: the clusters and units it fitted, as an estimate counts them,
/// whether the clip moved a unit value, and the fit of the values, `None` where `estimates`
/// gives none.
#[derive(Debug, Clone, PartialEq)]
pub struct Refit {
    pub clusters: u64,
    pub units: u64,
    pub clipped: bool,
    pub fit: Option<Fit>,
}

impl Source {
    /// The study of `records` whose stored plan is `text`. Its clusters are those `finish`
    /// measured: the data of each input comes from the `inputs` record and the rows
    /// (`held_by`), and the reserve clusters that ran from the `resize` record. `Err` (why)
    /// when the text does not parse, is not the plan the register record names, or a record
    /// the clusters need is missing or does not parse.
    pub fn read(text: &[u8], records: &[Record]) -> Result<Source, String> {
        let register =
            (records.iter().find_map(Register::of)).ok_or("it has no register record")?;
        let unparsed = "its stored plan does not parse";
        let raw = spec::parse(text).map_err(|_| unparsed)?;
        if raw.hash != register.spec_hash {
            return Err("its stored plan is not the one the study registered".into());
        }
        let (spec, _, hash, _) = typed(raw).map_err(|_| unparsed)?;
        let all = distinct_clusters(units::clusters(&spec, &hash), &held_by(records)?);
        let resize = resize::recorded(records).map_err(|_| "its resize record does not parse")?;
        let ran = resize::ran(&spec, &all, resize.as_ref());
        let clusters = measured(&spec, &all, &ran);
        Ok(Source {
            pairs: paired(&spec, records),
            spec,
            clusters,
        })
    }

    /// The fit `estimates` gives hypothesis `id` in group `stratum`, over the unit values,
    /// each clipped to `[lo, hi]` (infinite bounds clip nothing). `Err` when the plan has no
    /// hypothesis `id`.
    pub fn refit(&self, id: &str, stratum: &str, (lo, hi): (f64, f64)) -> Result<Refit, String> {
        let h = (self.spec.question.hypotheses.iter())
            .find(|h| h.id == id)
            .ok_or("its plan has no hypothesis its estimate names")?;
        let mut values = samples(&self.clusters, h, stratum, &self.pairs);
        let mut clipped = false;
        // Not `clamp`, which panics on a reversed range.
        for x in values.iter_mut().flatten() {
            let inside = x.max(lo).min(hi);
            clipped |= inside != *x;
            *x = inside;
        }
        Ok(Refit {
            clusters: values.len() as u64,
            units: values.iter().map(Vec::len).sum::<usize>() as u64,
            clipped,
            fit: fit(&values),
        })
    }
}

/// Each input of a study, in spec order, with the (exact, normalized) hashes of the data its
/// rows ran on. The `inputs` record names each input's exact hash, or none for one that was
/// absent, and every row names the pair of each input present, in the same order; all rows
/// name the same. A study with no row names no data, and each input stands alone
/// (`distinct_clusters`).
fn held_by(records: &[Record]) -> Result<InputHashes, String> {
    let Some(row) = own_rows(records).next() else {
        return Ok(vec![]);
    };
    let frozen = (records.iter().find_map(Inputs::of)).ok_or("it has rows and no inputs record")?;
    let other = "its rows and its inputs record name other data";
    if own_rows(records).any(|r| r.body["data_hashes"] != row.body["data_hashes"]) {
        return Err(other.into());
    }
    let named =
        Vec::<(String, String)>::deserialize(&row.body["data_hashes"]).map_err(|_| other)?;
    let mut named = named.into_iter();
    let mut out = Vec::new();
    for (path, exact) in frozen.inputs {
        let hashes = match exact {
            None => None,
            Some(exact) => Some(named.next().filter(|(x, _)| *x == exact).ok_or(other)?),
        };
        out.push((path, hashes));
    }
    if named.next().is_some() {
        return Err(other.into());
    }
    Ok(out)
}

/// The clusters a study measured, of `clusters`: each that is no reserve cluster, then the
/// reserve clusters whose rows ran (`ran`), in the order they are appended.
fn measured(spec: &Spec, clusters: &[Cluster], ran: &BTreeSet<&str>) -> Vec<Cluster> {
    let order = schedule::reserve_order(spec, clusters);
    (clusters.iter().filter(|c| c.role != Role::Reserve))
        .chain(order.into_iter().filter(|c| ran.contains(c.path.as_str())))
        .cloned()
        .collect()
}

/// The clusters of `role` in `stratum` (every stratum for the pooled group `""`), in the order
/// of `clusters`, each as the contrast's value for each picked unit that counts, and the count
/// of picked units that do not. A unit counts when every term pairs in at least `repeats`
/// repeats; its value is the weighted sum of the terms' means. A cluster with no unit that
/// counts is left out.
pub(crate) fn group(
    clusters: &[Cluster],
    role: Role,
    stratum: &str,
    outcome: &str,
    contrast: &BTreeMap<String, f64>,
    paired: &Paired,
    repeats: u32,
) -> (Vec<Vec<f64>>, u64) {
    let mut incomplete = 0;
    let samples = (clusters.iter())
        .filter(|c| c.role == role)
        .filter(|c| stratum.is_empty() || c.stratum == stratum)
        .map(|c| {
            let mut xs = Vec::new();
            for &i in &c.picked {
                let unit = units::key(&c.path, i);
                let x = (contrast.iter())
                    .map(|(cond, w)| {
                        let (mean, n) = paired.entry(&unit, cond, outcome)?;
                        (n >= repeats).then_some(w * mean)
                    })
                    .sum::<Option<f64>>();
                match x {
                    Some(x) => xs.push(x),
                    None => incomplete += 1,
                }
            }
            xs
        })
        .filter(|xs| !xs.is_empty())
        .collect();
    (samples, incomplete)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fit {
    /// The unweighted mean of the cluster means: what a follow-on's test targets.
    pub mean: f64,
    /// The variance of one unit: between plus within clusters.
    pub var: f64,
    /// Between over total; `None` when it is not defined.
    pub icc: Option<f64>,
}

/// A one-way analysis of variance over the clusters, `None` when the mean or the variance is
/// not finite, which is so for fewer than 2 values (a divisor of 0). With k clusters and n
/// values, MSB = SSB / (k - 1) and MSW = SSW / (n - k); `m0 = (n - sum m_c^2 / n) / (k - 1)`; the between-cluster variance
/// is `max(0, (MSB - MSW) / m0)`, `var` is it plus MSW and `icc` is it over `var`. With
/// k < 2 or n <= k a degree of freedom is missing: `var` is the sample variance of all
/// values and there is no icc.
fn fit(samples: &[Vec<f64>]) -> Option<Fit> {
    let k = samples.len() as f64;
    let n = samples.iter().map(Vec::len).sum::<usize>() as f64;
    let sum = |xs: &[f64]| xs.iter().sum::<f64>();
    let means: Vec<f64> = samples.iter().map(|xs| sum(xs) / xs.len() as f64).collect();
    let grand = samples.iter().map(|xs| sum(xs)).sum::<f64>() / n;
    let spread = |xs: &[f64], about: f64| xs.iter().map(|x| (x - about).powi(2)).sum::<f64>();
    let (var, icc) = if k < 2.0 || n <= k {
        (spread(&samples.concat(), grand) / (n - 1.0), None)
    } else {
        let ssb: f64 = (samples.iter().zip(&means))
            .map(|(xs, m)| xs.len() as f64 * (m - grand).powi(2))
            .sum();
        let ssw: f64 = (samples.iter().zip(&means))
            .map(|(xs, m)| spread(xs, *m))
            .sum();
        let squares: f64 = samples.iter().map(|xs| (xs.len() as f64).powi(2)).sum();
        let m0 = (n - squares / n) / (k - 1.0);
        let msw = ssw / (n - k);
        let between = ((ssb / (k - 1.0) - msw) / m0).max(0.0);
        let var = between + msw;
        (var, (var > 0.0).then(|| between / var))
    };
    let mean = sum(&means) / k;
    (mean.is_finite() && var.is_finite()).then_some(Fit { mean, var, icc })
}

/// One verdict per hypothesis, in spec order. A hypothesis with no claim gets `none`. A claim
/// is held in each stratum it lists, or in the pooled group `""` when it lists none. There
/// each confirmation cluster, then each reserve cluster in `clusters`, with a complete unit
/// gives one observation per bound (`claim::observation`), from its complete units only. A
/// unit is complete when every term of the contrast pairs in all `design.repeats` repeats.
/// - Confirmatory: per stratum, the confirmation clusters' observations in the order
///   `claim::order` draws from `seed(spec_hash, "test/<id>/<stratum>")`, then the reserve
///   clusters' in the order of `clusters`, then each bound's `claim::e_value`. The
///   hypothesis takes the least of them, an e-value under the union of the bounds' nulls, or 0
///   when a unit in scope is incomplete. `tree::decide` splits `level` by `alpha_share` and
///   passes each hypothesis it rejects.
/// - Exploratory: the claim passes when `claim::holds` for every bound in every stratum.
///
/// A study that `stopped` (why) before its reserve fails every claim, with that reason after
/// `stopped: ` and, in a test, e-value 0.
///
/// An e-value past the largest float is kept as that float: records and the stream hold
/// finite numbers, and a smaller e-value is still one.
pub fn verdicts(
    spec: &Spec,
    paired: &Paired,
    clusters: &[Cluster],
    spec_hash: &[u8; 32],
    level: Option<f64>,
    stopped: Option<&str>,
) -> Vec<Verdict> {
    let mut out: Vec<Verdict> = (spec.question.hypotheses.iter())
        .map(|h| judge(spec, h, paired, clusters, spec_hash))
        .collect();
    if let Some(why) = stopped {
        for v in out.iter_mut().filter(|v| v.basis != NONE) {
            v.verdict = "fail".into();
            v.e_value = v.e_value.map(|_| 0.0);
            v.reason = Some(format!("stopped: {why}"));
        }
    }
    if spec.study.mode != Mode::Confirmatory {
        return out;
    }
    let (claims, tree) = tree_of(spec, &out, level);
    match tree {
        Ok(tree) => {
            for (k, &i) in claims.iter().enumerate() {
                out[i].level = Some(tree.levels[k]);
                if tree.rejected[k] {
                    out[i].verdict = PASS.into();
                }
            }
        }
        Err(e) => {
            let why = format!("the study's level does not split over its claims: {e}");
            for &i in &claims {
                out[i].reason.get_or_insert_with(|| why.clone());
            }
        }
    }
    out
}

/// The e-value a confirmatory study closes its stream slot with, from its verdicts `vs`: the
/// tree's merged e-value over the tested ones at `level`, the largest `alpha_share` x e, or 0
/// when the tree cannot decide. It reaches 1 / `level` exactly when a claim passes, so the
/// stream counts a discovery only for a study with a passing claim. A verdict's e-value is
/// finite, and a share is at most 1, so this is finite too. `None` for an exploratory study.
pub fn study_e_value(spec: &Spec, vs: &[Verdict], level: Option<f64>) -> Option<f64> {
    let confirmatory = spec.study.mode == Mode::Confirmatory;
    confirmatory.then(|| tree_of(spec, vs, level).1.map_or(0.0, |t| t.e_value))
}

/// The tested verdicts of `vs`, by index, and the tree's decision over them at `level`, each
/// weighed by its hypothesis's `alpha_share`.
fn tree_of(
    spec: &Spec,
    vs: &[Verdict],
    level: Option<f64>,
) -> (Vec<usize>, Result<tree::Verdict, evidence::Error>) {
    let claims: Vec<usize> = (0..vs.len()).filter(|&i| vs[i].basis == TEST).collect();
    let share = |id: &str| {
        let h = spec.question.hypotheses.iter().find(|h| h.id == id);
        h.and_then(|h| h.alpha_share).unwrap_or(0.0)
    };
    let shares: Vec<f64> = claims.iter().map(|&i| share(&vs[i].hypothesis)).collect();
    let es: Vec<Option<f64>> = claims.iter().map(|&i| vs[i].e_value).collect();
    (claims, tree::decide(level.unwrap_or(0.0), &shares, &es))
}

const PASS: &str = "pass";
const NONE: &str = "none";
const TEST: &str = "test";

/// The verdict on `h` of `spec` before the level is split: a claim fails until the tree passes
/// it.
fn judge(
    spec: &Spec,
    h: &Hypothesis,
    paired: &Paired,
    clusters: &[Cluster],
    spec_hash: &[u8; 32],
) -> Verdict {
    let tested = spec.study.mode == Mode::Confirmatory;
    let repeats = spec.design.repeats;
    let scopes: Vec<Scope> = (claim_strata(h).into_iter())
        .map(|stratum| {
            let of = |role| {
                group(
                    clusters,
                    role,
                    stratum,
                    &h.outcome,
                    &h.contrast,
                    paired,
                    repeats,
                )
            };
            let ((first, a), (appended, b)) = (of(Role::Confirmation), of(Role::Reserve));
            Scope {
                stratum,
                first,
                appended,
                incomplete: a + b,
            }
        })
        .collect();
    let incomplete = scopes.iter().map(|s| s.incomplete).sum();
    let mut v = Verdict {
        hypothesis: h.id.clone(),
        verdict: NONE.into(),
        basis: NONE.into(),
        level: None,
        e_value: None,
        bounds: vec![],
        incomplete_units: incomplete,
        reason: None,
    };
    let Some(claim) = h.claim else {
        return v;
    };
    v.verdict = "fail".into();
    v.basis = if tested { TEST } else { "estimate" }.into();
    let results = match held(h, &claim, &scopes, spec_hash, tested) {
        Ok(results) => results,
        Err(e) => {
            v.reason = Some(format!("the claim cannot be tested: {e}"));
            v.e_value = tested.then_some(0.0);
            return v;
        }
    };
    let holds = results.iter().all(|(_, holds)| *holds);
    v.bounds = results.into_iter().map(|(b, _)| b).collect();
    if tested && incomplete > 0 {
        v.reason = Some(format!("incomplete units in scope: {incomplete}"));
        v.e_value = Some(0.0);
    } else if tested {
        let least = v.bounds.iter().filter_map(|b| b.value).reduce(f64::min);
        v.e_value = Some(least.unwrap_or(0.0));
    } else if holds {
        v.verdict = PASS.into();
    }
    v
}

/// One stratum of a claim's scope: its name, the values of the complete units of each
/// first-batch cluster and of each appended reserve cluster, and the count of incomplete units.
struct Scope<'a> {
    stratum: &'a str,
    first: Vec<Vec<f64>>,
    appended: Vec<Vec<f64>>,
    incomplete: u64,
}

/// Each bound of `claim` in each of `scopes`, with whether it holds by the point estimate. In
/// a test the bound's value is its e-value, else the mean of its observations. The
/// observations run over the first batch in its seeded order, then the appended clusters.
fn held(
    h: &Hypothesis,
    claim: &Claim,
    scopes: &[Scope],
    spec_hash: &[u8; 32],
    tested: bool,
) -> Result<Vec<(BoundResult, bool)>, evidence::Error> {
    let stat = (h.statistic.as_ref())
        .ok_or(evidence::Error::Parameter("statistic"))?
        .to_evidence();
    let mut out = Vec::new();
    for Scope {
        stratum,
        first,
        appended,
        ..
    } in scopes
    {
        let path = format!("test/{}/{stratum}", h.id);
        let order = claim::order(first.len(), &mut Rng::from_seed(seed(spec_hash, &path)));
        let samples: Vec<&Vec<f64>> = (order.iter().map(|&i| &first[i])).chain(appended).collect();
        for b in claim.bounds() {
            let obs = (samples.iter())
                .map(|xs| claim::observation(&stat, &b, xs))
                .collect::<Result<Vec<f64>, _>>()?;
            let value = if tested {
                Some(finite(claim::e_value(&stat, &b, &obs)?))
            } else {
                (!obs.is_empty()).then(|| obs.iter().sum::<f64>() / obs.len() as f64)
            };
            let side = if b.tail == Tail::Lower {
                "below"
            } else {
                "above"
            };
            let result = BoundResult {
                stratum: stratum.to_string(),
                side: side.into(),
                margin: b.margin,
                clusters: obs.len() as u64,
                value,
            };
            out.push((result, claim::holds(&stat, &b, &obs)?));
        }
    }
    Ok(out)
}

fn finite(e: f64) -> f64 {
    e.min(f64::MAX)
}

/// The strata a claim of `h` is held in: each it lists, once, in order; `""`, the pooled
/// group, when it lists none.
pub(crate) fn claim_strata(h: &Hypothesis) -> Vec<&str> {
    let mut strata: Vec<&str> = Vec::new();
    for s in &h.strata {
        if !strata.contains(&s.as_str()) {
            strata.push(s);
        }
    }
    if strata.is_empty() {
        strata.push("");
    }
    strata
}

/// SHA-256 hex of the canonical `{"question", "outcomes", "design"}` of `spec` followed by
/// `box_version`: what the analysis asked, and the box that ran it.
pub fn commit(spec: &Value, box_version: &str) -> Result<String, RunError> {
    let part = json!({
        "question": spec["question"], "outcomes": spec["outcomes"], "design": spec["design"],
    });
    let mut bytes = to_canonical(&part).map_err(|e| RunError::Spec(e.to_string()))?;
    bytes.extend_from_slice(box_version.as_bytes());
    Ok(sha256_hex(&bytes))
}

/// The `Instrument` of a run whose register record names `payload`: the environment its
/// `rows` name, and each model and calibration input of `spec` with a digest in `inputs`.
pub fn instrument(spec: &Spec, payload: &str, inputs: &InputHashes, rows: &[Record]) -> Instrument {
    let mut named = (rows.iter())
        .filter(|r| r.kind == "row")
        .map(|r| r.body["environment"].as_str());
    let environment = match named.next().flatten() {
        Some(first) if named.all(|e| e == Some(first)) => first,
        _ => "",
    };
    let mut built: Vec<(String, String, String)> = (spec.inputs.iter())
        .filter(|i| matches!(i.role, Role::Model | Role::Calibration))
        .filter_map(|i| {
            let (_, hashes) = inputs.iter().find(|(p, _)| *p == i.path)?;
            let role = json!(i.role).as_str()?.to_string();
            Some((i.path.clone(), role, hashes.as_ref()?.0.clone()))
        })
        .collect();
    built.sort();
    Instrument {
        payload: payload.into(),
        environment: environment.into(),
        inputs: built,
    }
}

/// The `Recheck` of a run of `spec` from its records, which carries the check of the study
/// `carried_from` names; `None` unless the plan declares `pins.deterministic`. A copy counts as
/// compared when its record's `same` is not `null`.
fn recheck(spec: &Spec, records: &[Record], carried_from: Option<String>) -> Option<Recheck> {
    let copies: Vec<&Record> = (records.iter())
        .filter(|r| r.kind == "row" && is_recheck(r))
        .collect();
    // The copies whose record says `same` is `same`, in the order they ran.
    let with = |same: Value| -> Vec<u64> {
        (copies.iter())
            .filter(|r| r.body["same"] == same)
            .filter_map(|r| r.row)
            .collect()
    };
    spec.pins.deterministic.then(|| {
        let uncompared = with(Value::Null);
        Recheck {
            rows: (copies.len() - uncompared.len()) as u64,
            mismatched: with(json!(false)),
            uncompared,
            carried_from,
        }
    })
}

/// When every scheduled row that runs has a record, add what the study's results still lack:
/// the `analysis` record first, which names the `instrument` and the `recheck`, and for a study
/// that re-sized, its spread (`resize::spreads`), and states how many records follow it, then
/// each estimate and each verdict it promises, and, for a study that holds a stream slot, the
/// close of that slot with the e-value its verdict records give (`study_e_value`). Last, a
/// study that re-sized releases the reserve data it never measured (`release`). A reserve
/// row runs when the `resize` record admits its cluster, and every one runs in a study with no
/// such record; the analysis reads the reserve clusters that ran, in the order they are
/// appended. A study the record stops fails its claims (`verdicts`). A call after a crash adds
/// only what is missing; a complete study gets nothing. A log whose analysis record another
/// box version wrote is refused when it lacks an estimate or a verdict. The close takes its
/// e-value from the signed records alone, so it is added whatever box wrote them.
pub fn finish(plan: &RunPlan) -> Result<(), RunError> {
    let dir = plan.home.study_dir(&plan.study_id);
    let log = dir.join("records.jsonl");
    let all = records::read_all(&log, &plan.key.verifying_key())?;
    let clusters = distinct_clusters(units::clusters(&plan.spec, &plan.spec_hash), &plan.inputs);
    let resize = resize::recorded(&all)?;
    let ran = resize::ran(&plan.spec, &clusters, resize.as_ref());
    let rowed: BTreeSet<u64> = (all.iter())
        .filter(|r| r.kind == "row")
        .filter_map(|r| r.row)
        .collect();
    let runs = |r: &&Row| resize::runs(r, &ran);
    if (plan.schedule.rows.iter().filter(runs)).any(|r| !rowed.contains(&r.index)) {
        return Ok(());
    }
    let measured = measured(&plan.spec, &clusters, &ran);
    let selection = all.iter().find_map(Selection::of);
    let pairs = paired(&plan.spec, &all);
    let wanted = estimates(
        &plan.spec,
        &plan.params,
        &pairs,
        &measured,
        selection.as_ref(),
    );
    let register = all.iter().find_map(Register::of);
    let alpha = register.as_ref().and_then(|r| r.alpha.clone());
    let level = alpha.as_ref().map(|a| a.level);
    let stopped = (resize.as_ref())
        .filter(|r| r.decision != CONTINUE)
        .map(|r| r.reason.as_deref().unwrap_or(resize::SHORT));
    let judged = verdicts(
        &plan.spec,
        &pairs,
        &measured,
        &plan.spec_hash,
        level,
        stopped,
    );
    let (_, value, hash, _) = stored_spec(&dir)?;
    if hash != plan.spec_hash {
        return Err(RunError::SpecChanged);
    }
    let version = env!("CARGO_PKG_VERSION");
    let mut have = BTreeSet::new();
    let mut recorded: BTreeMap<String, Verdict> = BTreeMap::new();
    match all.iter().position(|r| r.kind == ANALYSIS) {
        Some(at) => {
            let promised = Analysis::of(&all[at])
                .ok_or_else(|| RunError::Records("the analysis record does not parse".into()))?;
            let after = &all[at + 1..];
            have.extend(after.iter().filter_map(Estimate::of).map(|e| e.identity()));
            let verdicts = after.iter().filter_map(Verdict::of);
            recorded.extend(verdicts.map(|v| (v.hypothesis.clone(), v)));
            let missing = wanted.iter().any(|e| !have.contains(&e.identity()))
                || judged.iter().any(|v| !recorded.contains_key(&v.hypothesis));
            if promised.box_version != version && missing {
                return Err(RunError::Records(format!(
                    "the analysis record was written by box {}; box {version} does not add to it",
                    promised.box_version
                )));
            }
        }
        None => {
            let carried = register.as_ref().and_then(|r| r.carries.as_ref());
            let carried_from = carried.map(|c| c.study.clone());
            let (resize_sd, appended_sd) = (resize.as_ref())
                .map(|_| resize::spreads(&plan.spec, &pairs, &measured))
                .unzip();
            let body = Analysis {
                commit: commit(&value, version)?,
                box_version: version.into(),
                estimates: wanted.len() as u64,
                verdicts: judged.len() as u64,
                slot: alpha.as_ref().map(|a| a.slot),
                instrument: instrument(&plan.spec, &plan.payload_sha256, &plan.inputs, &all),
                recheck: recheck(&plan.spec, &all, carried_from),
                resize_sd,
                appended_sd,
            };
            append(plan, &log, ANALYSIS, &body)?;
        }
    }
    for e in wanted.iter().filter(|e| !have.contains(&e.identity())) {
        append(plan, &log, ESTIMATE, e)?;
    }
    for v in judged
        .iter()
        .filter(|v| !recorded.contains_key(&v.hypothesis))
    {
        append(plan, &log, VERDICT, v)?;
    }
    if let Some(a) = &alpha {
        let stream = Log::of(&plan.home, LogName::parse(&a.log)?);
        if !stream.closed(&plan.key.verifying_key(), &plan.study_id, a.slot)? {
            // The verdicts the log now holds: those recorded before, then those just added.
            let logged: Vec<Verdict> = (judged.iter())
                .map(|v| recorded.get(&v.hypothesis).unwrap_or(v).clone())
                .collect();
            let e = study_e_value(&plan.spec, &logged, level).unwrap_or(0.0);
            stream.close(&plan.key, &plan.study_id, a.slot, e)?;
        }
    }
    if resize.is_some() {
        let register = register.ok_or_else(|| RunError::NotRegistered(plan.study_id.clone()))?;
        release(plan, &register, &ran)?;
    }
    Ok(())
}

/// Free in the plan log the reserve data a study that re-sized never measured: each reserve
/// input whose cluster did not run (`ran`). No two reserve inputs hold the same data: a pinned
/// input is read at registration (E-HSH-001), and E-CNF-004 refuses the pair. One `release`
/// leaf, labelled as the study's late-data leaves are and naming its plan leaf's commitment;
/// none when there is no such data, and none again when the log holds it.
fn release(plan: &RunPlan, register: &Register, ran: &BTreeSet<&str>) -> Result<(), RunError> {
    let data: Vec<(String, String)> = (plan.spec.inputs.iter().zip(&plan.inputs))
        .filter(|(i, _)| i.role == Role::Reserve && !ran.contains(i.path.as_str()))
        .filter_map(|(_, (_, h))| h.clone())
        .collect();
    if data.is_empty() {
        return Ok(());
    }
    let log = PlanLog::open(&plan.home.root.join(&register.log))?;
    let commitment = (log.leaf(register.leaf)?)
        .and_then(|l| l.commitment)
        .ok_or_else(|| RunError::NotRegistered(plan.study_id.clone()))?;
    let done = (log.leaves()?.iter())
        .any(|l| l.kind == RELEASE && l.commitment.as_ref() == Some(&commitment));
    if !done {
        log.append_release(&own_label(&plan.study_id), &commitment, data, &plan.key)?;
    }
    Ok(())
}

/// Sign `body` as a record of `kind` in the study's log `log`, belonging to no row.
pub(crate) fn append(
    plan: &RunPlan,
    log: &Path,
    kind: &str,
    body: &impl Serialize,
) -> Result<Record, RunError> {
    let body = serde_json::to_value(body).map_err(|e| RunError::Records(e.to_string()))?;
    let draft = RecordDraft::new(&plan.study_id, kind, body);
    Ok(records::append(log, &plan.key, draft)?)
}

/// The clusters, without any whose input holds the same data (`prior::same_data`) as an
/// earlier cluster of the same role: the power gate counts such inputs as one cluster. An
/// input whose data was not read stands alone.
pub fn distinct_clusters(
    clusters: Vec<Cluster>,
    hashes: &[(String, Option<(String, String)>)],
) -> Vec<Cluster> {
    let mut data: HashMap<&str, Option<&(String, String)>> = HashMap::new();
    for (path, h) in hashes {
        data.entry(path).or_insert(h.as_ref());
    }
    let mut seen: HashMap<Role, Seen> = HashMap::new();
    (clusters.into_iter())
        .filter(|c| match data.get(c.path.as_str()).copied().flatten() {
            Some(h) => seen.entry(c.role).or_default().insert(h),
            None => true,
        })
        .collect()
}
