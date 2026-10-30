//! The cross-field checks. A rule row names one (`check = "named:<name>"`); its
//! `when` gates it like a generic rule. Each check gets the row's target pattern,
//! selects its nodes with it, and reports only at those nodes. A check skips what
//! is missing or mistyped: the generic rules report that.
use super::generic::{applies, diagnostic, join, join_key, plain_key, select};
use super::{BoxRecords, Check, NoRecords, Rule};
use crate::derived::Estimate;
use crate::diag::Diagnostic;
use crate::inputs::{self, Found};
use crate::power::required_units;
use crate::prior::{PriorUse, same_data};
use crate::schedule::ROW_CAP;
use crate::spec::model::SELECTED;
use crate::store::Store;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

pub struct CheckCtx {
    /// The study root. Every file the checks read lives under this folder.
    pub root: PathBuf,
    pub prior: Box<dyn PriorUse>,
    /// The highest isolation level this machine offers.
    pub machine_max_level: u8,
    /// The spec text the loader parsed, for checks that read comments.
    pub spec_text: String,
    pub records: Box<dyn BoxRecords>,
    /// The content store inputs resolve from first.
    pub store: Store,
}

impl CheckCtx {
    /// A context with no prior use and no records, for the folder `root`.
    pub fn empty(root: &Path) -> CheckCtx {
        CheckCtx {
            root: root.to_path_buf(),
            prior: Box::new(crate::prior::NoPriorUse),
            machine_max_level: 2,
            spec_text: String::new(),
            records: Box::new(NoRecords),
            store: Store {
                root: root.join("store"),
            },
        }
    }
}

/// Where a check fired, and text for the end of the rule's problem sentence.
struct Hit {
    path: String,
    detail: Option<String>,
}

fn hit(path: impl Into<String>) -> Hit {
    Hit {
        path: path.into(),
        detail: None,
    }
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
    ("units-declared", units_declared),
    ("row-cap", row_cap),
    ("selection-stage", selection_stage),
    ("condition-declared", condition_declared),
    ("param-shape", param_shape),
    ("claim-required", claim_required),
    ("hypothesis-shape", hypothesis_shape),
    ("hypothesis-strata", hypothesis_strata),
    ("alpha-shares", alpha_shares),
    ("budget-present", budget_present),
    ("route-valid", route_valid),
    ("follow-on-exists", follow_on_exists),
    ("depth-cap", depth_cap),
    ("seed-sets-disjoint", seed_sets_disjoint),
    ("trigger-declared", trigger_declared),
    ("isolation-reachable", isolation_reachable),
    ("input-path-safe", input_path_safe),
    ("input-path-nested", input_path_nested),
    ("input-hash-matches", input_hash_matches),
    ("secret-scan", secret_scan),
    ("comment-scan", comment_scan),
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
        .map(|(p, _)| hit(p))
        .collect()
}

/// For a pattern `<parent>.<leaf>`: each selected parent as (leaf path, parent node, leaf
/// node), the leaf `None` where the parent lacks it.
fn children<'a>(spec: &'a Value, pattern: &str) -> Vec<(String, &'a Value, Option<&'a Value>)> {
    let (parent, leaf) = pattern.rsplit_once('.').unwrap_or(("", pattern));
    let parents = if parent.is_empty() {
        vec![(String::new(), spec)]
    } else {
        nodes(spec, parent)
    };
    parents
        .into_iter()
        .map(|(p, el)| (join(&p, leaf), el, el.get(leaf)))
        .collect()
}

/// `children` with the leaf present.
fn with_parent<'a>(spec: &'a Value, pattern: &str) -> Vec<(String, &'a Value, &'a Value)> {
    children(spec, pattern)
        .into_iter()
        .filter_map(|(p, el, v)| Some((p, el, v?)))
        .collect()
}

/// For a pattern `<parent>.*`: each `*` key of each selected table, with its path. The key is
/// read from the table, since the path names an unsafe key `<key>`.
fn keys<'a>(spec: &'a Value, pattern: &str) -> Vec<(String, &'a str)> {
    let parent = pattern.strip_suffix(".*").unwrap_or(pattern);
    nodes(spec, parent)
        .into_iter()
        .filter_map(|(p, t)| Some((p, t.as_object()?)))
        .flat_map(|(p, m)| {
            m.keys()
                .filter(|k| plain_key(k))
                .map(move |k| (join_key(&p, k), k.as_str()))
        })
        .collect()
}

/// A hypothesis with a `claim`, well formed or not: E-HYP-003 judges its shape.
fn has_claim(hypothesis: &Value) -> bool {
    hypothesis.get("claim").is_some()
}

fn role(input: &Value) -> Option<&str> {
    input.get("role")?.as_str()
}

/// A selection or confirmation input: one cluster of the schedule.
fn is_cluster(input: &Value) -> bool {
    matches!(role(input), Some("selection" | "confirmation"))
}

/// The input's `(exact, normalized)` hashes, from the store first; `None` when there is no
/// file to read or its path is unsafe.
fn input_hashes(ctx: &CheckCtx, input: &Value) -> Option<(String, String)> {
    let path = input.get("path")?.as_str()?;
    let sha = input.get("sha256").and_then(Value::as_str).unwrap_or("");
    inputs::hashes(&inputs::resolve(&ctx.store, &ctx.root, path, sha))
}

// ---- files: one primitive guards every read ----

/// `rel` under the study root, or `None` when it could leave the folder. Every
/// segment must be one plain name; empty segments (`a//b`) are skipped. A `:`, a
/// backslash, a leading `/`, a segment ending in `.` or a space (Windows trims
/// them), or any symlink on the way makes it unsafe.
fn resolve(root: &Path, rel: &str) -> Option<PathBuf> {
    if rel.starts_with('/') || rel.contains(['\\', ':']) {
        return None;
    }
    let mut path = root.to_path_buf();
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
pub fn read_regular(path: &Path) -> Option<Vec<u8>> {
    read_file(path)
}

fn read_file(path: &Path) -> Option<Vec<u8>> {
    path.metadata()
        .ok()
        .filter(std::fs::Metadata::is_file)
        .and_then(|_| std::fs::read(path).ok())
}

/// `rel` under `root` when it stays inside the folder; see `resolve`.
pub fn input_path(root: &Path, rel: &str) -> Option<PathBuf> {
    resolve(root, rel)
}

fn input_path_safe(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    flag(spec, pat, |v| {
        v.as_str()
            .is_some_and(|rel| resolve(&ctx.root, rel).is_none())
    })
}

/// An input whose path lies inside another input's path (`data/a` and `data/a/b`).
fn input_path_nested(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    let all: Vec<(String, Vec<&str>)> = nodes(spec, pat)
        .into_iter()
        .filter_map(|(p, v)| {
            Some((
                p,
                v.as_str()?.split('/').filter(|s| !s.is_empty()).collect(),
            ))
        })
        .collect();
    all.iter()
        .filter(|(_, inner)| {
            all.iter().any(|(_, outer)| {
                !outer.is_empty() && outer.len() < inner.len() && inner.starts_with(outer)
            })
        })
        .map(|(p, _)| hit(p.clone()))
        .collect()
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
        match inputs::resolve(&ctx.store, &ctx.root, rel, want) {
            Found::Stored { .. } | Found::Unsafe => {}
            Found::File { exact, .. } if exact == want => {}
            Found::File { .. } | Found::Missing => out.push(hit(path)),
            Found::Damaged(why) => out.push(Hit {
                path,
                detail: Some(format!("The store entry for this pin is damaged: {why}.")),
            }),
        }
    }
    out
}

fn follow_on_exists(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    flag(spec, pat, |v| {
        v.as_str()
            .and_then(|rel| resolve(&ctx.root, rel))
            .is_some_and(|p| !p.is_file())
    })
}

fn data_fresh(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    let mut out = vec![];
    for (path, input, _) in with_parent(spec, pat) {
        if !is_cluster(input) {
            continue;
        }
        let Some((exact, normalized)) = input_hashes(ctx, input) else {
            continue;
        };
        let detail = match ctx.prior.used(&exact, &normalized) {
            Ok(None) => continue,
            Ok(Some(id)) => format!("The earlier entry is {id}."),
            Err(e) => format!("The plan log cannot be read: {e}."),
        };
        out.push(Hit {
            path,
            detail: Some(detail),
        });
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

/// The confirmation units are each confirmation input's units, up to `units_per_cluster`.
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
                    detail: Some(format!("Cannot compute power: design.{f} is absent.")),
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
    for (path, list) in nodes(spec, pat) {
        let Some(list) = list.as_array() else {
            continue;
        };
        let detail = if variance <= 0.0 {
            "The variance must be > 0.".to_string()
        } else {
            let have = list
                .iter()
                .filter(|i| role(i) == Some("confirmation"))
                .filter_map(|i| i.get("units")?.as_u64())
                .map(|u| u.min(per.unsigned_abs()))
                .fold(0, u64::saturating_add);
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

/// The source is `record:<hash>` of an estimate for a listed outcome that holds the stated variance.
fn variance_from_record(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    let outcomes = texts(spec, "outcomes[*].id");
    let stated = num(spec, "design.variance.value");
    let mut out = Vec::new();
    for (path, v) in nodes(spec, pat) {
        let Some(source) = v.as_str() else { continue };
        let Some(h) = source.strip_prefix("record:") else {
            out.push(hit(path));
            continue;
        };
        let fault = match ctx.records.find(h) {
            Err(e) => Some(format!("The box cannot read its records: {e}")),
            Ok(None) => Some("No record in this box has this hash.".to_string()),
            Ok(Some(r)) => match Estimate::of(&r) {
                None => Some("The record is not an estimate.".to_string()),
                Some(e) if !outcomes.contains(&e.outcome.as_str()) => {
                    Some("The record is for an outcome this plan does not list.".to_string())
                }
                Some(e) if stated.is_some_and(|s| s != e.var) => Some(format!(
                    "The record holds variance {}, and the plan states {}.",
                    e.var,
                    stated.unwrap_or_default()
                )),
                Some(_) => None,
            },
        };
        if let Some(detail) = fault {
            out.push(Hit {
                path,
                detail: Some(detail),
            });
        }
    }
    out
}

/// At `inputs`: no confirmation input. At `inputs[*].<field>`: a confirmation input holds a
/// selection input's data: the same exact hash, or the same normalized hash (text that differs
/// only in case and spacing). An empty normalized hash (not UTF-8 text) matches nothing.
fn selection_confirmation_disjoint(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    if !pat.contains('.') {
        return flag(spec, pat, |v| {
            v.as_array()
                .is_some_and(|a| !a.iter().any(|i| role(i) == Some("confirmation")))
        });
    }
    let selection: Vec<(String, String)> = nodes(spec, "inputs[*]")
        .into_iter()
        .filter(|(_, i)| role(i) == Some("selection"))
        .filter_map(|(_, i)| input_hashes(ctx, i))
        .collect();
    let shared = |h: &(String, String)| selection.iter().any(|s| same_data(s, h));
    with_parent(spec, pat)
        .into_iter()
        .filter(|(_, input, _)| role(input) == Some("confirmation"))
        .filter(|(_, input, _)| input_hashes(ctx, input).is_some_and(|h| shared(&h)))
        .map(|(path, _, _)| hit(path))
        .collect()
}

/// At `inputs`: no selection or confirmation input, so the schedule has no rows. At
/// `inputs[*].units`: such an input does not declare its units. At `inputs[*].stratum`: such an
/// input has no stratum, or `""`, while `design.stratum` names strata.
fn units_declared(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    let Some((_, field)) = pat.rsplit_once('.') else {
        return flag(spec, pat, |v| {
            v.as_array().is_some_and(|a| !a.iter().any(is_cluster))
        });
    };
    let strata = text(spec, "design.stratum").is_some_and(|s| !s.is_empty());
    children(spec, pat)
        .into_iter()
        .filter(|(_, input, v)| {
            is_cluster(input)
                && match field {
                    "units" => v.is_none(),
                    "stratum" => strata && v.is_none_or(|s| s.as_str() == Some("")),
                    _ => false,
                }
        })
        .map(|(path, _, _)| hit(path))
        .collect()
}

/// The conditions each unit of a cluster with this role runs: the reference and its stage's
/// list. `None` when the role has no stage: a calibration or model input, or a selection input
/// with no selection stage.
fn stage_size(spec: &Value, role: &str) -> Option<u64> {
    let list = match role {
        "selection" => list(spec, "design.selection.conditions")?,
        "confirmation" => list(spec, "design.confirmation.conditions")?,
        _ => return None,
    };
    Some(1 + list.len() as u64)
}

/// The schedule would hold more than `ROW_CAP` rows: each cluster's units, up to the units per
/// cluster at the target, times the conditions its stage runs, times the repeats. Counted
/// before anything is built, so a typo cannot exhaust the memory of `check`.
fn row_cap(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    let Some(repeats) = val(spec, "design.repeats").and_then(Value::as_u64) else {
        return vec![];
    };
    let clusters: Vec<(u64, u64)> = nodes(spec, "inputs[*]")
        .into_iter()
        .filter_map(|(_, i)| Some((i.get("units")?.as_u64()?, stage_size(spec, role(i)?)?)))
        .collect();
    let mut out = vec![];
    for (path, per) in nodes(spec, pat) {
        let Some(per) = per.as_u64() else { continue };
        let rows = clusters
            .iter()
            .fold(0u128, |n, (units, size)| {
                n.saturating_add(u128::from((*units).min(per)) * u128::from(*size))
            })
            .saturating_mul(u128::from(repeats));
        if rows > u128::from(ROW_CAP) {
            out.push(Hit {
                path,
                detail: Some(format!("It would hold {rows} rows; the cap is {ROW_CAP}.")),
            });
        }
    }
    out
}

/// The stages, at each target (`condition-declared` checks the names the stages list):
/// - `design.selection`: a stage, and no selection input.
/// - `inputs[*].role`: a selection input, and no stage.
/// - `design.selection.conditions`: fewer than two candidates.
/// - `design.selection.metric`: no outcome id.
/// - `design.conditions[*].id`: a condition named `selected`.
///
/// A mistyped stage counts as present.
fn selection_stage(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    let staged = val(spec, "design.selection").is_some();
    let has_input = |inputs: &Vec<Value>| inputs.iter().any(|i| role(i) == Some("selection"));
    match pat {
        "design.selection" => match list(spec, "inputs") {
            Some(inputs) if !has_input(inputs) => flag(spec, pat, Value::is_object),
            _ => vec![],
        },
        "inputs[*].role" => flag(spec, pat, |v| !staged && v.as_str() == Some("selection")),
        "design.selection.conditions" => {
            flag(spec, pat, |v| v.as_array().is_some_and(|a| a.len() < 2))
        }
        "design.selection.metric" => {
            let outcomes = texts(spec, "outcomes[*].id");
            flag(spec, pat, |v| {
                v.as_str().is_some_and(|m| !outcomes.contains(&m))
            })
        }
        "design.conditions[*].id" => flag(spec, pat, |v| v.as_str() == Some(SELECTED)),
        _ => vec![],
    }
}

const CONTRAST_KEYS: &str = "question.hypotheses[*].contrast.*";

/// A condition name its place may not use. The reference runs in every stage, so no stage
/// list or contrast names it; `selected` names the selection stage's pick.
/// - `design.reference`: a declared condition.
/// - `design.selection.conditions[*]`: a declared condition.
/// - `design.confirmation.conditions[*]`: a declared condition, or `selected` with a selection
///   stage.
/// - `question.hypotheses[*].contrast.*`, each key: a condition the confirmation stage lists,
///   `selected` only with a selection stage. Each term needs that stage's paired values. An
///   undeclared condition the stage lists is the stage list's fault, reported there.
///
/// With no declared condition ids, `non-empty` reports that; with no confirmation list, the
/// generic rules do.
fn condition_declared(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    let staged = val(spec, "design.selection").is_some();
    let usable: Vec<&str> = if pat == CONTRAST_KEYS {
        if list(spec, "design.confirmation.conditions").is_none() {
            return vec![];
        }
        let runs = texts(spec, "design.confirmation.conditions[*]");
        runs.into_iter()
            .filter(|c| *c != SELECTED || staged)
            .collect()
    } else {
        let declared = texts(spec, "design.conditions[*].id");
        if declared.is_empty() {
            return vec![];
        }
        let pick = staged && pat == "design.confirmation.conditions[*]";
        declared
            .into_iter()
            .chain(pick.then_some(SELECTED))
            .collect()
    };
    let reference = text(spec, "design.reference").filter(|_| pat != "design.reference");
    let bad = |c: &str| !usable.contains(&c) || Some(c) == reference;
    if pat == CONTRAST_KEYS {
        return keys(spec, pat)
            .into_iter()
            .filter(|(_, k)| bad(k))
            .map(|(p, _)| hit(p))
            .collect();
    }
    flag(spec, pat, |v| v.as_str().is_some_and(bad))
}

/// A param value is a string or a number.
fn param_shape(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    flag(spec, pat, |v| !(v.is_string() || v.is_number()))
}

// ---- hypotheses ----

/// A hypothesis with no claim. Its rule's `when` limits it to confirmatory studies.
fn claim_required(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    children(spec, pat)
        .into_iter()
        .filter(|(_, h, claim)| h.is_object() && claim.is_none())
        .map(|(p, _, _)| hit(p))
        .collect()
}

/// A value read by a shape rule: `None` when it is mistyped or holds an unknown key (a generic
/// rule reports that), `Some(Err(()))` when it is malformed.
type Shape<T> = Option<Result<T, ()>>;

/// The one entry of a table whose keys are all in `allowed`.
fn one_key<'a>(v: &'a Value, allowed: &[&str]) -> Shape<(&'a str, &'a Value)> {
    let m = v.as_object()?;
    if !m.keys().all(|k| allowed.contains(&k.as_str())) {
        return None;
    }
    Some(match m.iter().next() {
        Some((k, x)) if m.len() == 1 => Ok((k.as_str(), x)),
        _ => Err(()),
    })
}

/// `[a, b]` with a < b.
fn ordered_pair(v: &Value) -> Shape<(f64, f64)> {
    let xs: Vec<f64> = v
        .as_array()?
        .iter()
        .map(Value::as_f64)
        .collect::<Option<_>>()?;
    Some(match *xs.as_slice() {
        [a, b] if a < b => Ok((a, b)),
        _ => Err(()),
    })
}

/// The margins of a claim: `below m`, `above m` or `within [a, b]`.
fn margins(claim: &Value) -> Shape<Vec<f64>> {
    match one_key(claim, &["below", "above", "within"])? {
        Ok(("within", v)) => Some(ordered_pair(v)?.map(|(a, b)| vec![a, b])),
        Ok((_, v)) => Some(Ok(vec![v.as_f64()?])),
        Err(()) => Some(Err(())),
    }
}

/// The range a statistic declares for a cluster mean; `None` for a quantile.
fn mean_range(statistic: &Value) -> Shape<Option<(f64, f64)>> {
    match one_key(statistic, &["mean", "quantile"])? {
        Ok(("mean", v)) => Some(ordered_pair(v)?.map(Some)),
        Ok((_, q)) => Some(if open_unit(q.as_f64()?) {
            Ok(None)
        } else {
            Err(())
        }),
        Err(()) => Some(Err(())),
    }
}

/// Each hypothesis, at each target:
/// - `<h>.outcome`: no listed outcome id.
/// - `<h>.contrast`: an empty table. `<h>.contrast.*`: a weight of 0. A non-finite weight is
///   the finite rule's.
/// - `<h>.claim`: not one of `below m`, `above m` or `within [a, b]` with a < b; or a margin at
///   or outside the statistic's mean range.
/// - `<h>.statistic`: absent with a claim; or not one of `mean [lo, hi]` with lo < hi or
///   `quantile q` with 0 < q < 1. A statistic without a claim is checked too.
fn hypothesis_shape(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    let at = |keep: &dyn Fn(&Value, Option<&Value>) -> bool| -> Vec<Hit> {
        children(spec, pat)
            .into_iter()
            .filter(|(_, h, v)| keep(h, *v))
            .map(|(p, _, _)| hit(p))
            .collect()
    };
    match pat.rsplit_once('.').map_or(pat, |(_, leaf)| leaf) {
        "outcome" if list(spec, "outcomes").is_some() => {
            let outcomes = texts(spec, "outcomes[*].id");
            flag(spec, pat, |v| {
                v.as_str().is_some_and(|o| !outcomes.contains(&o))
            })
        }
        "contrast" => flag(spec, pat, |v| v.as_object().is_some_and(|m| m.is_empty())),
        "*" => flag(spec, pat, |v| v.as_f64() == Some(0.0)),
        "claim" => at(&|h, claim| match claim.and_then(margins) {
            Some(Err(())) => true,
            Some(Ok(ms)) => match h.get("statistic").and_then(mean_range) {
                Some(Ok(Some((lo, hi)))) => ms.iter().any(|&m| m <= lo || m >= hi),
                _ => false,
            },
            None => false,
        }),
        "statistic" => at(&|h, statistic| match statistic {
            None => has_claim(h),
            Some(s) => matches!(mean_range(s), Some(Err(()))),
        }),
        _ => vec![],
    }
}

/// A listed stratum that no confirmation input names.
fn hypothesis_strata(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    let Some(inputs) = list(spec, "inputs") else {
        return vec![];
    };
    let held: Vec<&str> = inputs
        .iter()
        .filter(|i| role(i) == Some("confirmation"))
        .filter_map(|i| i.get("stratum")?.as_str())
        .collect();
    flag(spec, pat, |v| {
        v.as_str().is_some_and(|s| !held.contains(&s))
    })
}

fn share_in_range(x: f64) -> bool {
    x > 0.0 && x <= 1.0
}

/// The split of a confirmatory study's level, over the hypotheses with a claim (E-CNF-006
/// reports the others):
/// - `<h>.alpha_share`: absent, or outside (0, 1].
/// - `question.hypotheses`: every share is in (0, 1], and they do not sum to 1. The sum may
///   miss 1 by 1e-9 for rounding: 0.7 + 0.2 + 0.1 is 0.9999999999999999.
fn alpha_shares(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    if pat.ends_with(".alpha_share") {
        return children(spec, pat)
            .into_iter()
            .filter(|(_, h, share)| {
                has_claim(h) && share.is_none_or(|s| s.as_f64().is_some_and(|x| !share_in_range(x)))
            })
            .map(|(p, _, _)| hit(p))
            .collect();
    }
    let mut out = vec![];
    for (path, hypotheses) in nodes(spec, pat) {
        let Some(hypotheses) = hypotheses.as_array() else {
            continue;
        };
        let shares: Option<Vec<f64>> = hypotheses
            .iter()
            .filter(|h| has_claim(h))
            .map(|h| {
                h.get("alpha_share")?
                    .as_f64()
                    .filter(|x| share_in_range(*x))
            })
            .collect();
        let Some(sum) = shares
            .filter(|s| !s.is_empty())
            .map(|s| s.iter().sum::<f64>())
        else {
            continue;
        };
        if (sum - 1.0).abs() > 1e-9 {
            out.push(Hit {
                path,
                detail: Some(format!("The shares sum to {sum}.")),
            });
        }
    }
    out
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

/// The `when` that runs a follow-on whatever the result.
const DONE: &str = "done";

/// A `follow_on` array is empty under route follow-on, or a `when` is neither `done` nor
/// `<id>.pass` or `<id>.fail` for a hypothesis with a claim.
fn route_valid(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    let claimed: Vec<&str> = nodes(spec, "question.hypotheses[*]")
        .into_iter()
        .filter(|(_, h)| has_claim(h))
        .filter_map(|(_, h)| h.get("id")?.as_str())
        .collect();
    let follow = text(spec, "scope.end_of_run.route") == Some("follow-on");
    let whens = pat.ends_with("[*].when");
    flag(spec, pat, |v| match v {
        Value::Array(a) => !whens && follow && a.is_empty(),
        Value::String(w) => {
            whens
                && w != DONE
                && !matches!(w.rsplit_once('.'), Some((id, "pass" | "fail")) if claimed.contains(&id))
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

// ---- secrets ----

/// One hit per finding at `path`. The detail names the kind, never the text.
fn secret_hits(path: &str, text: &str) -> Vec<Hit> {
    crate::scan::scan_str(text, crate::scan::builtin())
        .into_iter()
        .map(|kind| Hit {
            path: path.to_string(),
            detail: Some(format!("Kind: {kind}.")),
        })
        .collect()
}

fn secret_scan(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    nodes(spec, pat)
        .into_iter()
        .filter_map(|(p, v)| Some((p, v.as_str()?)))
        .flat_map(|(p, s)| secret_hits(&p, s))
        .collect()
}

/// Each line from its first `#` on, at `line N`. The frozen copy keeps comments. A `#` inside
/// a string can only repeat a report of a value `secret_scan` already flags.
fn comment_scan(_: &Value, ctx: &CheckCtx, _: &str) -> Vec<Hit> {
    ctx.spec_text
        .lines()
        .enumerate()
        .filter_map(|(i, l)| Some((i + 1, &l[l.find('#')?..])))
        .flat_map(|(n, comment)| secret_hits(&format!("line {n}"), comment))
        .collect()
}
