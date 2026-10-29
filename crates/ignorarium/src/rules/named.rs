//! The cross-field checks. A rule row names one (`check = "named:<name>"`); its
//! `when` gates it like a generic rule. Each check gets the row's target pattern,
//! selects its nodes with it, and reports only at those nodes. A check skips what
//! is missing or mistyped: the generic rules report that.
use super::generic::{applies, diagnostic, join, select};
use super::{Check, Rule};
use crate::canon::sha256_hex;
use crate::diag::Diagnostic;
use crate::power::required_units;
use crate::prior::{PriorUse, normalized_sha256};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

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

/// Spec, context, and the target pattern of the rule row that runs it.
type NamedFn = fn(&Value, &CheckCtx, &str) -> Vec<Hit>;

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
        for h in run(spec, ctx, &rule.path) {
            let problem = match h.detail {
                Some(d) => format!("{} {d}", rule.problem),
                None => rule.problem.clone(),
            };
            out.push(diagnostic(rule, h.path, problem));
        }
    }
    out
}

// ---- reading the spec: one path syntax, `select`'s ----

/// The existing nodes a pattern selects, with their concrete paths.
fn nodes<'a>(spec: &'a Value, pattern: &str) -> Vec<(String, &'a Value)> {
    select(spec, pattern)
        .into_iter()
        .filter_map(|(p, v)| Some((p, v?)))
        .collect()
}

fn val<'a>(spec: &'a Value, path: &str) -> Option<&'a Value> {
    nodes(spec, path).into_iter().next().map(|(_, v)| v)
}

fn text<'a>(spec: &'a Value, path: &str) -> Option<&'a str> {
    val(spec, path)?.as_str()
}

fn num(spec: &Value, path: &str) -> Option<f64> {
    val(spec, path)?.as_f64()
}

fn list<'a>(spec: &'a Value, path: &str) -> Option<&'a Vec<Value>> {
    val(spec, path)?.as_array()
}

fn texts<'a>(spec: &'a Value, pattern: &str) -> Vec<&'a str> {
    nodes(spec, pattern)
        .into_iter()
        .filter_map(|(_, v)| v.as_str())
        .collect()
}

/// One hit per selected node that satisfies `bad`.
fn flag(spec: &Value, pattern: &str, bad: impl Fn(&Value) -> bool) -> Vec<Hit> {
    nodes(spec, pattern)
        .into_iter()
        .filter(|(_, v)| bad(v))
        .flat_map(|(p, _)| hit(p))
        .collect()
}

/// For a pattern `<parent>.<leaf>`: each selected leaf as (leaf path, its parent node, leaf node).
fn with_parent<'a>(spec: &'a Value, pattern: &str) -> Vec<(String, &'a Value, &'a Value)> {
    let (parent, leaf) = pattern.rsplit_once('.').unwrap_or(("", pattern));
    let parents = if parent.is_empty() {
        vec![(String::new(), spec)]
    } else {
        nodes(spec, parent)
    };
    parents
        .into_iter()
        .filter_map(|(p, el)| Some((join(&p, leaf), el, el.get(leaf)?)))
        .collect()
}

// ---- files: one primitive guards every read ----

/// `rel` under the study folder, or `None` when it could leave the folder. Every
/// segment must be one plain name; empty segments (`a//b`) are skipped. A `:`, a
/// backslash, a leading `/`, a segment ending in `.` or a space (Windows trims
/// them), or any symlink on the way makes it unsafe.
fn resolve(ctx: &CheckCtx, rel: &str) -> Option<PathBuf> {
    if rel.starts_with('/') || rel.contains(['\\', ':']) {
        return None;
    }
    let mut path = ctx.study_dir.clone();
    let mut any = false;
    for part in rel.split('/').filter(|c| !c.is_empty()) {
        let mut parts = Path::new(part).components();
        // One Normal component per segment: no `..`, root or prefix, so the result stays inside.
        let plain = matches!(
            (parts.next(), parts.next()),
            (Some(Component::Normal(_)), None)
        );
        if !plain || part.ends_with(['.', ' ']) {
            return None;
        }
        path.push(part);
        any = true;
        if path.symlink_metadata().is_ok_and(|m| m.is_symlink()) {
            return None;
        }
    }
    any.then_some(path)
}

/// The bytes of a regular file; never opens a pipe or device.
fn read_file(path: &Path) -> Option<Vec<u8>> {
    path.metadata()
        .ok()
        .filter(std::fs::Metadata::is_file)
        .and_then(|_| std::fs::read(path).ok())
}

fn input_path_safe(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    flag(spec, pat, |v| {
        v.as_str().is_some_and(|rel| resolve(ctx, rel).is_none())
    })
}

fn input_hash_matches(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    let mut out = vec![];
    for (path, input, sha) in with_parent(spec, pat) {
        let (Some(want), Some(rel)) = (sha.as_str(), input.get("path").and_then(Value::as_str))
        else {
            continue;
        };
        if want.is_empty() {
            continue;
        }
        let Some(file) = resolve(ctx, rel) else {
            continue;
        };
        if read_file(&file).map(|b| sha256_hex(&b)).as_deref() != Some(want) {
            out.extend(hit(path));
        }
    }
    out
}

fn follow_on_exists(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    flag(spec, pat, |v| {
        v.as_str()
            .and_then(|rel| resolve(ctx, rel))
            .is_some_and(|p| !p.is_file())
    })
}

fn data_fresh(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    let mut out = vec![];
    for (path, input, rel) in with_parent(spec, pat) {
        let role = input.get("role").and_then(Value::as_str);
        if !matches!(role, Some("selection" | "confirmation")) {
            continue;
        }
        let Some(bytes) = rel
            .as_str()
            .and_then(|r| resolve(ctx, r))
            .and_then(|p| read_file(&p))
        else {
            continue;
        };
        // Not UTF-8 text: no normalized hash exists, so pass an empty one.
        let normalized = std::str::from_utf8(&bytes)
            .map(normalized_sha256)
            .unwrap_or_default();
        if let Some(id) = ctx.prior.used(&sha256_hex(&bytes), &normalized) {
            out.push(Hit {
                path,
                detail: Some(format!("The earlier entry is {id}.")),
            });
        }
    }
    out
}

// ---- design ----

fn label_allowed(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    flag(spec, pat, |v| v.as_str().is_some_and(|l| l != "signal"))
}

fn sesoi_required(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    flag(spec, pat, |v| v.as_f64().is_some_and(|x| x <= 0.0))
}

fn open_unit(x: f64) -> bool {
    x > 0.0 && x < 1.0
}

/// Alpha, power and icc come from the defaults step. If one is absent the gate
/// cannot be computed, so it fails closed. A mistyped one is skipped: the type rule reports it.
fn power_met(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    let (Some(variance), Some(sesoi), Some(per)) = (
        num(spec, "design.variance.value"),
        num(spec, "design.sesoi"),
        val(spec, "design.units_per_cluster").and_then(Value::as_i64),
    ) else {
        return vec![];
    };
    let fields = ["alpha", "power", "icc"].map(|f| (f, val(spec, &format!("design.{f}"))));
    if sesoi <= 0.0
        || fields
            .iter()
            .any(|(_, v)| v.is_some_and(|v| !v.is_number()))
    {
        return vec![];
    }
    let absent: Vec<_> = fields.iter().filter(|(_, v)| v.is_none()).collect();
    if !absent.is_empty() {
        return nodes(spec, pat)
            .into_iter()
            .filter(|(_, v)| v.is_array())
            .flat_map(|(path, _)| {
                absent.iter().map(move |(f, _)| Hit {
                    path: path.clone(),
                    detail: Some(format!("cannot compute power: design.{f} is absent")),
                })
            })
            .collect();
    }
    let [alpha, power, icc] = fields.map(|(_, v)| v.and_then(Value::as_f64).unwrap_or_default());
    // The range rules report these.
    if !open_unit(alpha) || !open_unit(power) || !(0.0..1.0).contains(&icc) {
        return vec![];
    }
    let per = per.max(0);
    let m = u32::try_from(per.max(1)).unwrap_or(u32::MAX);
    let mut out = vec![];
    for (path, clusters) in nodes(spec, pat) {
        let Some(clusters) = clusters.as_array() else {
            continue;
        };
        let detail = if variance <= 0.0 {
            "The variance must be > 0.".to_string()
        } else {
            let have = (clusters.len() as u64).saturating_mul(per.unsigned_abs());
            let need = required_units(alpha, power, variance.sqrt(), sesoi, m, icc);
            if have >= need {
                continue;
            }
            format!("{have} confirmation units; the power gate needs {need}.")
        };
        out.push(Hit {
            path,
            detail: Some(detail),
        });
    }
    out
}

fn variance_from_record(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    flag(spec, pat, |v| {
        v.as_str().is_some_and(|s| !s.starts_with("record:"))
    })
}

/// The array is empty, or one element is also a selection cluster.
fn selection_confirmation_disjoint(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    let selection = list(spec, "design.selection.clusters").map_or(&[][..], Vec::as_slice);
    let elements = pat.ends_with("[*]");
    flag(spec, pat, |v| {
        if elements {
            selection.contains(v)
        } else {
            v.as_array().is_some_and(Vec::is_empty)
        }
    })
}

fn test_allowed(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    flag(spec, pat, |v| {
        v.as_str().is_some_and(|t| t != "paired-mean")
    })
}

// ---- scope ----

fn budget_present(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    let keys = ["cpu_hours", "wall_hours", "money_usd"];
    flag(spec, pat, |v| {
        let Some(budget) = v.as_object() else {
            return false;
        };
        let value = |k: &&str| budget.get(*k);
        let mistyped = keys.iter().filter_map(value).any(|x| !x.is_number());
        let set = keys
            .iter()
            .filter_map(value)
            .any(|x| x.as_f64().is_some_and(|x| x > 0.0));
        !mistyped && !set
    })
}

/// A `follow_on` array is empty under route follow-on, or a `when` is not
/// `<hypothesis id>.pass` or `.fail`.
fn route_valid(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    let hypotheses = texts(spec, "question.hypotheses[*].id");
    let follow = text(spec, "scope.end_of_run.route") == Some("follow-on");
    let whens = pat.ends_with("[*].when");
    flag(spec, pat, |v| match v {
        Value::Array(a) => !whens && follow && a.is_empty(),
        Value::String(w) => {
            whens
                && !matches!(w.rsplit_once('.'), Some((id, "pass" | "fail")) if hypotheses.contains(&id))
        }
        _ => false,
    })
}

fn isolation_reachable(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    flag(spec, pat, |v| {
        v.as_f64()
            .is_some_and(|x| x > f64::from(ctx.machine_max_level))
    })
}

// ---- mutations ----

fn depth_cap(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    flag(spec, pat, |v| v.as_f64().is_some_and(|x| x > 2.0))
}

fn seed_sets_disjoint(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    flag(spec, pat, |v| {
        let seed = |k: &str| v.get(k).and_then(Value::as_str);
        matches!((seed("spawn"), seed("report")),
            (Some(a), Some(b)) if a.starts_with(b) || b.starts_with(a))
    })
}

fn trigger_declared(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    if list(spec, "outcomes").is_none() {
        return vec![];
    }
    let outcomes = texts(spec, "outcomes[*].id");
    flag(spec, pat, |v| {
        v.as_str().is_some_and(|m| !outcomes.contains(&m))
    })
}
