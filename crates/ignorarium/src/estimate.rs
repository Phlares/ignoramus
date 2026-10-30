//! Defaults for absent spec keys, and the cost estimate.
use crate::machine::Machine;
use crate::schedule::Row;
use crate::spec::model::{Mode, SELECTED, Spec};
use serde_json::{Map, Value, json};

/// The only place that holds default values: path, and the value given the spec.
/// Computes a default from the whole spec.
type Make = fn(&Value) -> Value;

const DEFAULTS: &[(&str, Make)] = &[
    ("design.alpha", |_| json!(0.005)),
    ("design.power", |_| json!(0.8)),
    ("design.repeats", |_| json!(1)),
    ("design.pick", |_| json!("seeded")),
    ("mutations.depth_cap", |_| json!(2)),
    ("scope.min_isolation", |_| json!(1)),
    ("access.blind", |v| {
        json!(v.pointer("/study/mode").and_then(Value::as_str) == Some("confirmatory"))
    }),
];

/// Fills each absent key. Returns the paths filled and their values, in table order.
/// A parent that exists but is not a table is skipped: the type rule reports it.
pub fn apply_defaults(v: &mut Value) -> Vec<(String, Value)> {
    let mut filled = Vec::new();
    for (path, value) in DEFAULTS {
        let value = value(v);
        if insert_absent(v, path, &value) {
            filled.push((path.to_string(), value));
        }
    }
    filled
}

/// `[{path, value}]` for the filled defaults.
pub fn defaults_json(filled: &[(String, Value)]) -> Value {
    filled
        .iter()
        .map(|(path, value)| json!({ "path": path, "value": value }))
        .collect()
}

/// Sets `path` to `value` when absent, creating absent parent tables. True when set.
fn insert_absent(node: &mut Value, path: &str, value: &Value) -> bool {
    let Some(map) = node.as_object_mut() else {
        return false;
    };
    match path.split_once('.') {
        None if map.contains_key(path) => false,
        None => {
            map.insert(path.to_string(), value.clone());
            true
        }
        Some((head, rest)) => {
            let child = map.entry(head).or_insert_with(|| Value::Object(Map::new()));
            insert_absent(child, rest, value)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Estimate {
    pub rows: u64,
    pub rows_per_condition: Vec<(String, u64)>,
    pub cpu_hours: f64,
    pub wall_hours: f64,
    pub money_usd: f64,
    pub label_earnable: String,
    pub is_long_job: bool,
}

/// The cost of `rows`, the schedule `run` writes, counted row by row. Rows count per
/// declared condition, and per `selected` when the confirmation stage runs the pick.
pub fn estimate(spec: &Spec, machine: &Machine, rows: &[Row]) -> Estimate {
    let d = &spec.design;
    let picks = d.confirmation.conditions.iter().any(|c| c == SELECTED);
    let rows_per_condition: Vec<(String, u64)> = d
        .conditions
        .iter()
        .map(|c| c.id.as_str())
        .chain(picks.then_some(SELECTED))
        .map(|id| {
            let n = rows.iter().filter(|r| r.condition == id).count();
            (id.to_string(), n as u64)
        })
        .collect();
    let n = rows.len() as u64;
    let cpu_seconds = n as f64 * d.row_seconds;
    let cpus = f64::from(machine.cpus.max(1));
    let cpu_hours = cpu_seconds / 3600.0;
    let wall_hours = cpu_hours / cpus;
    Estimate {
        rows: n,
        rows_per_condition,
        cpu_hours,
        wall_hours,
        money_usd: wall_hours * machine.rate_usd_per_hour,
        label_earnable: match spec.study.mode {
            Mode::Exploratory => "signal",
            Mode::Confirmatory => "confirmed",
        }
        .into(),
        is_long_job: cpu_seconds / cpus > f64::from(machine.long_job_minutes) * 60.0,
    }
}
