//! The cross-field checks. A rule row names one (`check = "named:<name>"`); its
//! `when` gates it like a generic rule, and its path is where it is documented.
//! A check reads what it needs and skips what is missing or mistyped: the generic
//! rules report those.
use super::generic::{applies, diagnostic};
use super::{Check, Rule};
use crate::canon::sha256_hex;
use crate::diag::Diagnostic;
use crate::power::required_units;
use crate::prior::{PriorUse, normalized_sha256};
use serde_json::Value;
use std::path::PathBuf;

pub struct CheckCtx {
    /// Every file the checks read lives under this folder.
    pub study_dir: PathBuf,
    pub prior: Box<dyn PriorUse>,
    /// The highest isolation level this machine offers.
    pub machine_max_level: u8,
}

/// Where a check fired, and text for the end of the rule's problem sentence.
struct Hit {
    path: String,
    detail: Option<String>,
}

fn hit(path: impl Into<String>) -> Vec<Hit> {
    vec![Hit {
        path: path.into(),
        detail: None,
    }]
}

type NamedFn = fn(&Value, &CheckCtx) -> Vec<Hit>;

const NAMED: &[(&str, NamedFn)] = &[
    ("label-allowed-for-mode", label_allowed),
    ("sesoi-required", sesoi_required),
    ("power-met", power_met),
    ("variance-from-record", variance_from_record),
    (
        "selection-confirmation-disjoint",
        selection_confirmation_disjoint,
    ),
    ("data-fresh", data_fresh),
    ("confirmatory-test-allowed", test_allowed),
    ("budget-present", budget_present),
    ("route-valid", route_valid),
    ("follow-on-exists", follow_on_exists),
    ("depth-cap", depth_cap),
    ("seed-sets-disjoint", seed_sets_disjoint),
    ("trigger-declared", trigger_declared),
    ("isolation-reachable", isolation_reachable),
    ("input-path-safe", input_path_safe),
    ("input-hash-matches", input_hash_matches),
];

fn lookup(name: &str) -> Option<NamedFn> {
    NAMED.iter().find(|(n, _)| *n == name).map(|(_, f)| *f)
}

pub(super) fn is_known(name: &str) -> bool {
    lookup(name).is_some()
}

/// Every violation of the named rules, in rule-file order.
pub fn check_named(spec: &Value, rules: &[Rule], ctx: &CheckCtx) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for rule in rules.iter().filter(|r| applies(r, spec)) {
        let Check::Named(name) = &rule.check else {
            continue;
        };
        let Some(run) = lookup(name) else { continue };
        for h in run(spec, ctx) {
            let problem = match h.detail {
                Some(d) => format!("{} {d}", rule.problem),
                None => rule.problem.clone(),
            };
            out.push(diagnostic(rule, h.path, problem));
        }
    }
    out
}

// ---- reading the spec ----

fn str_at<'a>(spec: &'a Value, ptr: &str) -> Option<&'a str> {
    spec.pointer(ptr)?.as_str()
}

fn num_at(spec: &Value, ptr: &str) -> Option<f64> {
    spec.pointer(ptr)?.as_f64()
}

/// A number with a default for an absent key. A mistyped value gives `None`.
fn num_or(spec: &Value, ptr: &str, default: f64) -> Option<f64> {
    match spec.pointer(ptr) {
        None => Some(default),
        Some(v) => v.as_f64(),
    }
}

fn array_at<'a>(spec: &'a Value, ptr: &str) -> Option<&'a Vec<Value>> {
    spec.pointer(ptr)?.as_array()
}

/// Strings under `key` of each element of an array, skipping elements without one.
fn keys_of<'a>(spec: &'a Value, ptr: &str, key: &str) -> Option<Vec<&'a str>> {
    Some(
        array_at(spec, ptr)?
            .iter()
            .filter_map(|e| e.get(key)?.as_str())
            .collect(),
    )
}

// ---- files: one primitive guards every read ----

/// `rel` under the study folder, or `None` when it could leave the folder:
/// empty, absolute, drive-prefixed, containing `..` or `\`, or passing through a symlink.
fn resolve(ctx: &CheckCtx, rel: &str) -> Option<PathBuf> {
    let drive = matches!(rel.as_bytes(), [d, b':', ..] if d.is_ascii_alphabetic());
    if rel.is_empty()
        || rel.starts_with('/')
        || rel.contains('\\')
        || drive
        || rel.split('/').any(|c| c == "..")
    {
        return None;
    }
    let mut path = ctx.study_dir.clone();
    for part in rel.split('/').filter(|c| !c.is_empty() && *c != ".") {
        path.push(part);
        if path.symlink_metadata().is_ok_and(|m| m.is_symlink()) {
            return None;
        }
    }
    Some(path)
}

/// (spec path, file path) of every file the checks may read.
fn file_refs(spec: &Value) -> Vec<(String, &str)> {
    let each = |ptr: &str, key: &str, base: &str| -> Vec<(String, &str)> {
        let Some(items) = array_at(spec, ptr) else {
            return vec![];
        };
        items
            .iter()
            .enumerate()
            .filter_map(|(i, e)| Some((format!("{base}[{i}].{key}"), e.get(key)?.as_str()?)))
            .collect()
    };
    let mut refs = each("/inputs", "path", "inputs");
    refs.extend(each(
        "/scope/end_of_run/follow_on",
        "spec",
        "scope.end_of_run.follow_on",
    ));
    refs
}

fn input_path_safe(spec: &Value, ctx: &CheckCtx) -> Vec<Hit> {
    file_refs(spec)
        .into_iter()
        .filter(|(_, rel)| resolve(ctx, rel).is_none())
        .flat_map(|(path, _)| hit(path))
        .collect()
}

fn input_hash_matches(spec: &Value, ctx: &CheckCtx) -> Vec<Hit> {
    let Some(inputs) = array_at(spec, "/inputs") else {
        return vec![];
    };
    let mut out = vec![];
    for (i, input) in inputs.iter().enumerate() {
        let (Some(want), Some(rel)) = (
            input.get("sha256").and_then(Value::as_str),
            input.get("path").and_then(Value::as_str),
        ) else {
            continue;
        };
        if want.is_empty() {
            continue;
        }
        let Some(file) = resolve(ctx, rel) else {
            continue;
        };
        if std::fs::read(file).map(|b| sha256_hex(&b)).ok().as_deref() != Some(want) {
            out.extend(hit(format!("inputs[{i}].sha256")));
        }
    }
    out
}

fn follow_on_exists(spec: &Value, ctx: &CheckCtx) -> Vec<Hit> {
    let Some(items) = array_at(spec, "/scope/end_of_run/follow_on") else {
        return vec![];
    };
    let mut out = vec![];
    for (i, item) in items.iter().enumerate() {
        let Some(rel) = item.get("spec").and_then(Value::as_str) else {
            continue;
        };
        if resolve(ctx, rel).is_some_and(|p| !p.is_file()) {
            out.extend(hit(format!("scope.end_of_run.follow_on[{i}].spec")));
        }
    }
    out
}

fn data_fresh(spec: &Value, ctx: &CheckCtx) -> Vec<Hit> {
    let Some(inputs) = array_at(spec, "/inputs") else {
        return vec![];
    };
    let mut out = vec![];
    for (i, input) in inputs.iter().enumerate() {
        let role = input.get("role").and_then(Value::as_str);
        let rel = input.get("path").and_then(Value::as_str);
        let (Some("selection" | "confirmation"), Some(rel)) = (role, rel) else {
            continue;
        };
        let Some(bytes) = resolve(ctx, rel).and_then(|p| std::fs::read(p).ok()) else {
            continue;
        };
        // Not UTF-8 text: no normalized hash exists, so pass an empty one.
        let normalized = std::str::from_utf8(&bytes)
            .map(normalized_sha256)
            .unwrap_or_default();
        if let Some(id) = ctx.prior.used(&sha256_hex(&bytes), &normalized) {
            out.push(Hit {
                path: format!("inputs[{i}].path"),
                detail: Some(format!("The earlier entry is {id}.")),
            });
        }
    }
    out
}

// ---- design ----

fn label_allowed(spec: &Value, _: &CheckCtx) -> Vec<Hit> {
    match str_at(spec, "/study/label") {
        Some(l) if l != "signal" => hit("study.label"),
        _ => vec![],
    }
}

fn sesoi_required(spec: &Value, _: &CheckCtx) -> Vec<Hit> {
    match num_at(spec, "/design/sesoi") {
        Some(x) if x <= 0.0 => hit("design.sesoi"),
        _ => vec![],
    }
}

fn open_unit(x: f64) -> bool {
    x > 0.0 && x < 1.0
}

fn power_met(spec: &Value, _: &CheckCtx) -> Vec<Hit> {
    let (Some(variance), Some(sesoi), Some(units_per_cluster)) = (
        num_at(spec, "/design/variance/value"),
        num_at(spec, "/design/sesoi"),
        spec.pointer("/design/units_per_cluster")
            .and_then(Value::as_i64),
    ) else {
        return vec![];
    };
    let (Some(alpha), Some(power), Some(icc), Some(clusters)) = (
        num_or(spec, "/design/alpha", 0.005),
        num_or(spec, "/design/power", 0.8),
        num_or(spec, "/design/icc", 0.0),
        array_at(spec, "/design/confirmation/clusters"),
    ) else {
        return vec![];
    };
    // E-CNF-001 and the range rules report these.
    if sesoi <= 0.0 || !open_unit(alpha) || !open_unit(power) || !(0.0..1.0).contains(&icc) {
        return vec![];
    }
    let detail = if variance <= 0.0 {
        "The variance must be > 0.".to_string()
    } else {
        let per = units_per_cluster.max(0);
        let have = clusters.len() as u64 * per as u64;
        let m = u32::try_from(per.max(1)).unwrap_or(u32::MAX);
        let need = required_units(alpha, power, variance.sqrt(), sesoi, m, icc);
        if have >= need {
            return vec![];
        }
        format!("{have} confirmation units; the power gate needs {need}.")
    };
    vec![Hit {
        path: "design.confirmation.clusters".into(),
        detail: Some(detail),
    }]
}

fn variance_from_record(spec: &Value, _: &CheckCtx) -> Vec<Hit> {
    match str_at(spec, "/design/variance/source") {
        Some(s) if !s.starts_with("record:") => hit("design.variance.source"),
        _ => vec![],
    }
}

fn selection_confirmation_disjoint(spec: &Value, _: &CheckCtx) -> Vec<Hit> {
    let Some(confirmation) = array_at(spec, "/design/confirmation/clusters") else {
        return vec![];
    };
    if confirmation.is_empty() {
        return hit("design.confirmation.clusters");
    }
    let selection = array_at(spec, "/design/selection/clusters").map_or(&[][..], Vec::as_slice);
    confirmation
        .iter()
        .enumerate()
        .filter(|(_, c)| selection.contains(c))
        .flat_map(|(i, _)| hit(format!("design.confirmation.clusters[{i}]")))
        .collect()
}

fn test_allowed(spec: &Value, _: &CheckCtx) -> Vec<Hit> {
    match str_at(spec, "/design/test") {
        Some(t) if t != "paired-mean" => hit("design.test"),
        _ => vec![],
    }
}

// ---- scope ----

fn budget_present(spec: &Value, _: &CheckCtx) -> Vec<Hit> {
    let Some(Value::Object(budget)) = spec.pointer("/scope/budget") else {
        return vec![];
    };
    let keys = ["cpu_hours", "wall_hours", "money_usd"];
    if keys
        .iter()
        .any(|k| budget.get(*k).is_some_and(|v| !v.is_number()))
    {
        return vec![];
    }
    let set = keys.iter().any(|k| {
        budget
            .get(*k)
            .and_then(Value::as_f64)
            .is_some_and(|x| x != 0.0)
    });
    if set { vec![] } else { hit("scope.budget") }
}

fn route_valid(spec: &Value, _: &CheckCtx) -> Vec<Hit> {
    let mut out = vec![];
    let follow = array_at(spec, "/scope/end_of_run/follow_on");
    if str_at(spec, "/scope/end_of_run/route") == Some("follow-on")
        && follow.is_some_and(Vec::is_empty)
    {
        out.extend(hit("scope.end_of_run.follow_on"));
    }
    let hypotheses = keys_of(spec, "/question/hypotheses", "id").unwrap_or_default();
    let valid = |when: &str| matches!(when.rsplit_once('.'), Some((id, "pass" | "fail")) if hypotheses.contains(&id));
    for (i, item) in follow.into_iter().flatten().enumerate() {
        if item
            .get("when")
            .and_then(Value::as_str)
            .is_some_and(|w| !valid(w))
        {
            out.extend(hit(format!("scope.end_of_run.follow_on[{i}].when")));
        }
    }
    out
}

fn isolation_reachable(spec: &Value, ctx: &CheckCtx) -> Vec<Hit> {
    match num_at(spec, "/scope/min_isolation") {
        Some(x) if x > f64::from(ctx.machine_max_level) => hit("scope.min_isolation"),
        _ => vec![],
    }
}

// ---- mutations ----

fn depth_cap(spec: &Value, _: &CheckCtx) -> Vec<Hit> {
    match num_at(spec, "/mutations/depth_cap") {
        Some(x) if x > 2.0 => hit("mutations.depth_cap"),
        _ => vec![],
    }
}

fn seed_sets_disjoint(spec: &Value, _: &CheckCtx) -> Vec<Hit> {
    match (
        str_at(spec, "/mutations/seeds/spawn"),
        str_at(spec, "/mutations/seeds/report"),
    ) {
        (Some(a), Some(b)) if a.starts_with(b) || b.starts_with(a) => hit("mutations.seeds"),
        _ => vec![],
    }
}

fn trigger_declared(spec: &Value, _: &CheckCtx) -> Vec<Hit> {
    let (Some(outcomes), Some(triggers)) = (
        keys_of(spec, "/outcomes", "id"),
        array_at(spec, "/mutations/triggers"),
    ) else {
        return vec![];
    };
    triggers
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            t.get("metric")
                .and_then(Value::as_str)
                .is_some_and(|m| !outcomes.contains(&m))
        })
        .flat_map(|(i, _)| hit(format!("mutations.triggers[{i}].metric")))
        .collect()
}
