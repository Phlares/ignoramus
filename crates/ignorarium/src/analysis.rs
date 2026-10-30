//! Reading the rows back: each condition's value for a unit, paired with the reference
//! condition's value for the same unit and repeat.
use crate::canon::{sha256_hex, to_canonical};
use crate::derived::{ANALYSIS, Analysis, Candidate, ESTIMATE, Estimate, Selection, contrast_key};
use crate::estimate::apply_defaults;
use crate::records::{self, Record, RecordDraft};
use crate::runner::{RunError, RunPlan};
use crate::schedule::SELECTION_STAGE;
use crate::spec::model::{Direction, Outcome, Paired as Rule, Role, SELECTED, Spec};
use crate::units::{self, Cluster};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Paired values by (unit, condition, outcome id).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Paired {
    by_key: BTreeMap<(String, String, String), f64>,
}

impl Paired {
    /// The mean over the repeats that pair; `None` when no repeat does.
    pub fn get(&self, unit: &str, condition: &str, outcome: &str) -> Option<f64> {
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

/// Pair every `ok` row record's values with the reference condition's row for the same
/// unit and repeat. `Difference` is condition minus reference; `Percent` is
/// 100 x (condition / reference - 1), and no pair when the reference is not above zero.
pub fn paired(spec: &Spec, rows: &[Record]) -> Paired {
    // (unit, repeat, condition) -> the row's values
    let mut cells: BTreeMap<(&str, u64, &str), &serde_json::Value> = BTreeMap::new();
    for r in rows.iter().filter(|r| r.kind == "row") {
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
            .map(|(k, (sum, n))| (k, sum / f64::from(n)))
            .filter(|(_, v)| v.is_finite())
            .collect(),
    }
}

/// The selection stage's pick. A selection unit (a unit of a `selection` row record) is
/// complete when it has a paired metric value for every candidate. Each candidate's mean is
/// over the complete units; the best mean in the metric outcome's direction wins, and a tie
/// goes to the first listed. `Err` when the plan has no stage or candidate, its metric is not
/// an outcome, no unit is complete, or a mean is not finite.
pub fn select(spec: &Spec, rows: &[Record]) -> Result<Selection, String> {
    let stage = (spec.design.selection.as_ref()).ok_or("the plan has no selection stage")?;
    let outcome = (spec.outcomes.iter().find(|o| o.id == stage.metric))
        .ok_or("the selection metric is not an outcome of the plan")?;
    let units: BTreeSet<&str> = rows
        .iter()
        .filter(|r| r.kind == "row" && r.body["stage"] == SELECTION_STAGE)
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

/// Each declared condition's params as a JSON table, by condition id: `{}` for none.
pub fn declared_params(spec: &Spec) -> BTreeMap<String, Value> {
    (spec.design.conditions.iter())
        .map(|c| (c.id.clone(), json!(&c.params)))
        .collect()
}

/// One estimate per hypothesis, per group: each stratum the hypothesis lists, then the pooled
/// group `""` (alone when it lists none). A group holds the confirmation clusters of its
/// stratum. A unit's value is the contrast over its paired values, and counts only when
/// every term has one. A `selected` row is recorded under `selected`, so its value is read
/// there, while the key names the pick. `params` holds each condition's params, by id. A hypothesis whose outcome is unknown, or whose contrast needs a pick that
/// `selection` does not give, has none, and neither has a group with fewer than 2 values.
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
        let (key, key_hash) = contrast_key(outcome, &reference, &terms);
        let mut groups: Vec<&str> = h.strata.iter().map(String::as_str).collect();
        groups.push("");
        for stratum in groups {
            let samples: Vec<Vec<f64>> = (clusters.iter())
                .filter(|c| c.role == Role::Confirmation)
                .filter(|c| stratum.is_empty() || c.stratum == stratum)
                .map(|c| cluster_values(c, outcome, &h.contrast, paired))
                .filter(|xs| !xs.is_empty())
                .collect();
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

/// The contrast's value for each picked unit of `c` that has a paired value for every term.
fn cluster_values(
    c: &Cluster,
    outcome: &Outcome,
    contrast: &BTreeMap<String, f64>,
    paired: &Paired,
) -> Vec<f64> {
    (c.picked.iter())
        .filter_map(|&i| {
            let unit = units::key(&c.path, i);
            (contrast.iter())
                .map(|(cond, w)| Some(w * paired.get(&unit, cond, &outcome.id)?))
                .sum::<Option<f64>>()
        })
        .collect()
}

struct Fit {
    /// The unweighted mean of the cluster means: what a follow-on's test targets.
    mean: f64,
    /// The variance of one unit: between plus within clusters.
    var: f64,
    /// Between over total; `None` when it is not defined.
    icc: Option<f64>,
}

/// A one-way analysis of variance over the clusters, `None` when the mean or the variance is
/// not finite, which is so for fewer than 2 values (a divisor of 0). With k clusters and n values, MSB = SSB / (k - 1) and
/// MSW = SSW / (n - k); `m0 = (n - sum m_c^2 / n) / (k - 1)`; the between-cluster variance
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

/// When every scheduled row has a record, add what the study's results still lack: the
/// `analysis` record first, which states how many records follow it, then each estimate it
/// promises. A call after a crash adds only what is missing; a complete study gets nothing.
pub fn finish(plan: &RunPlan) -> Result<(), RunError> {
    let dir = plan.home.study_dir(&plan.study_id);
    let log = dir.join("records.jsonl");
    let all = records::read_all(&log, &plan.key.verifying_key())?;
    let rowed: BTreeSet<u64> = (all.iter())
        .filter(|r| r.kind == "row")
        .filter_map(|r| r.row)
        .collect();
    if plan.schedule.rows.iter().any(|r| !rowed.contains(&r.index)) {
        return Ok(());
    }
    let selection = all.iter().find_map(Selection::of);
    let clusters = units::clusters(&plan.spec, &plan.spec_hash);
    let mut wanted = estimates(
        &plan.spec,
        &declared_params(&plan.spec),
        &paired(&plan.spec, &all),
        &clusters,
        selection.as_ref(),
    );
    let mut seen = BTreeSet::new();
    wanted.retain(|e| seen.insert(identity(e)));
    let mut have = BTreeSet::new();
    match all.iter().position(|r| r.kind == ANALYSIS) {
        Some(at) => {
            Analysis::of(&all[at])
                .ok_or_else(|| RunError::Records("the analysis record does not parse".into()))?;
            let after = all[at + 1..].iter().filter_map(Estimate::of);
            have.extend(after.map(|e| identity(&e)));
        }
        None => {
            let mut spec = crate::spec::load(&dir.join("spec.toml"))
                .map_err(|e| RunError::Spec(e.to_string()))?
                .value;
            apply_defaults(&mut spec);
            let version = env!("CARGO_PKG_VERSION");
            let body = Analysis {
                commit: commit(&spec, version)?,
                box_version: version.into(),
                estimates: wanted.len() as u64,
                verdicts: 0,
                slot: None,
            };
            append(plan, &log, ANALYSIS, &body)?;
        }
    }
    for e in wanted.iter().filter(|e| !have.contains(&identity(e))) {
        append(plan, &log, ESTIMATE, e)?;
    }
    Ok(())
}

/// What makes an estimate distinct: its hypothesis, contrast and stratum.
fn identity(e: &Estimate) -> (String, String, String) {
    (e.hypothesis.clone(), e.key_hash.clone(), e.stratum.clone())
}

fn append(plan: &RunPlan, log: &Path, kind: &str, body: &impl Serialize) -> Result<(), RunError> {
    let draft = RecordDraft {
        study: plan.study_id.clone(),
        row: None,
        kind: kind.into(),
        body: serde_json::to_value(body).map_err(|e| RunError::Records(e.to_string()))?,
    };
    records::append(log, &plan.key, draft)?;
    Ok(())
}
