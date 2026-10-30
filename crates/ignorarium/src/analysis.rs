//! Reading the rows back: each condition's value for a unit, paired with the reference
//! condition's value for the same unit and repeat.
use crate::derived::{Candidate, Selection};
use crate::records::Record;
use crate::spec::model::{Direction, Paired as Rule, Spec};
use std::collections::{BTreeMap, BTreeSet};

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
        .filter(|r| r.kind == "row" && r.body["stage"] == "selection")
        .filter_map(|r| r.body["unit"].as_str())
        .collect();
    let pairs = paired(spec, rows);
    let complete: Vec<Vec<f64>> = units
        .iter()
        .filter_map(|u| {
            (stage.conditions.iter())
                .map(|c| pairs.get(u, c, &outcome.id))
                .collect()
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
        let mean = complete.iter().map(|v| v[i]).sum::<f64>() / n;
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
        selected: best.id.clone(),
        complete_units: complete.len() as u64,
        incomplete_units: (units.len() - complete.len()) as u64,
        candidates,
    })
}
