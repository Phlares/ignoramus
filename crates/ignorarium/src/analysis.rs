//! Reading the rows back: each condition's value for a unit, paired with the reference
//! condition's value for the same unit and repeat.
use crate::records::Record;
use crate::spec::model::{Paired as Rule, Spec};
use std::collections::BTreeMap;

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
