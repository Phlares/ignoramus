//! The spawn engine at run. At the selection boundary each declared trigger bets on the
//! selection data; a trigger that fires admits its children while the spec estimate of the
//! rows stays within the budget. One `trigger` record per trigger holds the outcome, and only
//! the children those records admit run.
use crate::analysis::{self, Paired, group, strata_of};
use crate::derived::{SPAWN_STOP, Selection, Skipped, SpawnStop, TRIGGER, Trigger};
use crate::estimate;
use crate::machine;
use crate::records::{self, Record};
use crate::runner::{RunError, RunPlan};
use crate::schedule::{self, Child, Row, SPAWN_STAGE};
use crate::seed::seed;
use crate::spec::model::{DeclaredTrigger, Role, SELECTED, Scalar, Spec};
use crate::units::{self, Cluster};
use evidence::claim;
use evidence::rng::Rng;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// What the boundary decides for one trigger: the body of its `trigger` record.
pub type Fired = Trigger;

pub const NOT_FIRED: &str = "not-fired";
pub const SAME_AS_PARENT: &str = "same-as-parent";
pub const BUDGET: &str = "budget";
pub const INCOMPLETE: &str = "incomplete";
pub const NO_PICK: &str = "no-pick";

/// Each declared trigger of `plan`, in spec order, at the selection boundary: the one its
/// `trigger` record in `records` holds, or else one decided now from the selection rows in
/// `records`. A trigger's value is the least stopped e-value (`claim::stopped`, stop at
/// `threshold_e`) over its claim's bounds and strata, each from one observation per selection
/// cluster in the order drawn from `seed(spec_hash, "<seeds.spawn><id>/<stratum>")`; an
/// incomplete selection unit in scope makes it 0. `selected` in a contrast is the pick of the
/// study's `selection` record; a trigger that needs the pick (`wants_pick`) when the records
/// hold none gets 0 and reason `no-pick`. A trigger fires when its value reaches `threshold_e`. A fired
/// trigger admits each of its children in declared order, unless the child's value is the one
/// its parent holds (`same-as-parent`), or the spec estimate of the rows that are no child
/// rows, the rows of the children admitted before it and its own does not stay within the
/// budget (`budget`). A child an earlier trigger admitted is admitted again at no cost.
pub fn evaluate(plan: &RunPlan, records: &[Record]) -> Result<Vec<Fired>, RunError> {
    let spec = &plan.spec;
    let have = recorded(records)?.unwrap_or_default();
    let pick = picked(records);
    let pick = pick.as_deref();
    let machine = machine::load(&plan.home.root).map_err(|e| RunError::Io(e.to_string()))?;
    let fits = |taken: &[&str], child: &str| {
        let tried = [taken, &[child]].concat();
        estimate::admitted(spec, &machine, &plan.schedule.rows, &tried).contains(&child)
    };
    let children = schedule::children(spec);
    let clusters =
        analysis::distinct_clusters(units::clusters(spec, &plan.spec_hash), &plan.inputs);
    let pairs = analysis::paired(spec, records);
    // The schedule ids of the children admitted so far, in order.
    let mut taken: Vec<&str> = vec![];
    let mut out = vec![];
    for t in spec.mutations.declared() {
        let declared = children.iter().filter(|c| c.triggers.contains(&t.id));
        if let Some(r) = have.iter().find(|r| r.id == t.id) {
            for c in declared {
                if r.admitted.contains(&run_id(c, pick)) && !taken.contains(&&c.id[..]) {
                    taken.push(&c.id);
                }
            }
            out.push(r.clone());
            continue;
        }
        // With no pick, a trigger that needs one cannot bet, and admits no child.
        let (e_value, reason) = match pick {
            None if wants_pick(t, &children) => (0.0, Some(NO_PICK.into())),
            _ => bet(plan, t, &clusters, &pairs, pick),
        };
        let fired = reason.is_none() && e_value >= t.threshold_e;
        let mut f = Trigger {
            id: t.id.clone(),
            e_value,
            threshold: t.threshold_e,
            fired,
            reason,
            admitted: vec![],
            skipped: vec![],
        };
        for c in declared {
            let why = if !fired {
                Some(NOT_FIRED)
            } else if same_as_parent(&plan.params, c, pick) {
                Some(SAME_AS_PARENT)
            } else if taken.contains(&&c.id[..]) {
                None
            } else if fits(&taken, &c.id) {
                taken.push(&c.id);
                None
            } else {
                Some(BUDGET)
            };
            let child = run_id(c, pick);
            match why {
                None => f.admitted.push(child),
                Some(reason) => f.skipped.push(Skipped {
                    child,
                    reason: reason.into(),
                }),
            }
        }
        out.push(f);
    }
    Ok(out)
}

/// The value of trigger `t` and, when the bet cannot be read, why (its value is then 0).
fn bet(
    plan: &RunPlan,
    t: &DeclaredTrigger,
    clusters: &[Cluster],
    pairs: &Paired,
    pick: Option<&str>,
) -> (f64, Option<String>) {
    let mut contrast: BTreeMap<String, f64> = BTreeMap::new();
    for (c, w) in &t.contrast {
        let c = match (c.as_str(), pick) {
            (SELECTED, Some(p)) => p,
            (SELECTED, None) => return (0.0, Some(NO_PICK.into())),
            (c, _) => c,
        };
        *contrast.entry(c.into()).or_default() += w;
    }
    let stat = t.statistic.to_evidence();
    let mut least = f64::INFINITY;
    for stratum in strata_of(&t.strata) {
        let repeats = plan.spec.design.repeats;
        let (samples, incomplete) = group(
            clusters,
            Role::Selection,
            stratum,
            &t.outcome,
            &contrast,
            pairs,
            repeats,
        );
        if incomplete > 0 {
            return (0.0, Some(INCOMPLETE.into()));
        }
        let path = format!("{}{}/{stratum}", plan.spec.mutations.seeds.spawn, t.id);
        let order = claim::order(
            samples.len(),
            &mut Rng::from_seed(seed(&plan.spec_hash, &path)),
        );
        for b in t.claim.bounds() {
            let value = (order.iter())
                .map(|&i| claim::observation(&stat, &b, &samples[i]))
                .collect::<Result<Vec<f64>, _>>()
                .and_then(|obs| claim::stopped(&stat, &b, &obs, t.threshold_e));
            match value {
                Ok(v) => least = least.min(v),
                Err(e) => return (0.0, Some(format!("the trigger cannot be tested: {e}"))),
            }
        }
    }
    (analysis::finite(least), None)
}

/// A declared trigger needs the pick (`wants_pick`).
pub fn needs_pick(spec: &Spec) -> bool {
    let children = schedule::children(spec);
    (spec.mutations.declared()).any(|t| wants_pick(t, &children))
}

/// Trigger `t`'s contrast names `selected`, or a child of `children` it declares acts on it.
fn wants_pick(t: &DeclaredTrigger, children: &[Child]) -> bool {
    let acts = |c: &Child| c.parent == SELECTED && c.triggers.contains(&t.id);
    t.contrast.contains_key(SELECTED) || children.iter().any(acts)
}

/// The pick of the study's `selection` record, when it holds one.
fn picked(records: &[Record]) -> Option<String> {
    records.iter().find_map(Selection::of).map(|s| s.selected)
}

/// The condition `c` acts on: the pick for `selected`.
fn parent_of<'a>(c: &'a Child, pick: Option<&'a str>) -> &'a str {
    pick.filter(|_| c.parent == SELECTED).unwrap_or(&c.parent)
}

/// The id child `c` runs as: its schedule id with `selected` replaced by the pick.
pub fn run_id(c: &Child, pick: Option<&str>) -> String {
    schedule::child_id(parent_of(c, pick), &c.operator, &c.value)
}

/// The condition id child `c` runs as and its params: its parent's params (the pick's for
/// `selected`) with its operator set to its value.
pub fn job(params: &BTreeMap<String, Value>, c: &Child, pick: Option<&str>) -> (String, Value) {
    let mut own = params.get(parent_of(c, pick)).cloned().unwrap_or_default();
    if !own.is_object() {
        own = Value::Object(Default::default());
    }
    own[&c.operator] = serde_json::to_value(&c.value).unwrap_or_default();
    (run_id(c, pick), own)
}

/// The parent of `c` already holds its value, as the schedule compares values (`shown`).
fn same_as_parent(params: &BTreeMap<String, Value>, c: &Child, pick: Option<&str>) -> bool {
    let held = params
        .get(parent_of(c, pick))
        .and_then(|p| p.get(&c.operator));
    let held = held.and_then(|v| serde_json::from_value::<Scalar>(v.clone()).ok());
    held.is_some_and(|v| schedule::shown(&v) == schedule::shown(&c.value))
}

/// The declared child whose schedule id is `id`.
pub fn child_of(spec: &Spec, id: &str) -> Option<Child> {
    schedule::children(spec).into_iter().find(|c| c.id == id)
}

/// The study's `trigger` records, in log order; `None` when it holds none. `Err` when one does
/// not parse.
pub fn recorded(records: &[Record]) -> Result<Option<Vec<Fired>>, RunError> {
    let all = (records.iter().filter(|r| r.kind == TRIGGER))
        .map(|r| Trigger::of(r).ok_or_else(|| unparsed(TRIGGER)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((!all.is_empty()).then_some(all))
}

/// Each child of `spec` whose rows run, by schedule id, with the hash of the first `trigger`
/// record in `records` that admits it: every admitted child but those a `spawn-stop` record
/// names. `Err` when a trigger or spawn-stop record does not parse.
pub fn admitted(spec: &Spec, records: &[Record]) -> Result<BTreeMap<String, String>, RunError> {
    let pick = picked(records);
    let pick = pick.as_deref();
    let mut stopped = BTreeSet::new();
    for r in records.iter().filter(|r| r.kind == SPAWN_STOP) {
        stopped.extend(
            SpawnStop::of(r)
                .ok_or_else(|| unparsed(SPAWN_STOP))?
                .children,
        );
    }
    let children = schedule::children(spec);
    let mut out = BTreeMap::new();
    for r in records.iter().filter(|r| r.kind == TRIGGER) {
        let t = Trigger::of(r).ok_or_else(|| unparsed(TRIGGER))?;
        for c in &children {
            let id = run_id(c, pick);
            if t.admitted.contains(&id) && !stopped.contains(&id) {
                out.entry(c.id.clone()).or_insert_with(|| records::hash(r));
            }
        }
    }
    Ok(out)
}

/// The children whose rows run (`admitted`) with a row of `rows` that has no record in
/// `records`, by the id they run as.
pub fn unfinished(spec: &Spec, rows: &[Row], records: &[Record]) -> Result<Vec<String>, RunError> {
    let admitted = admitted(spec, records)?;
    let pick = picked(records);
    let done: BTreeSet<u64> = (records.iter())
        .filter(|r| r.kind == "row")
        .filter_map(|r| r.row)
        .collect();
    let open: BTreeSet<&str> = (rows.iter())
        .filter(|r| r.stage == SPAWN_STAGE && !done.contains(&r.index))
        .map(|r| r.condition.as_str())
        .collect();
    Ok((schedule::children(spec).iter())
        .filter(|c| admitted.contains_key(&c.id) && open.contains(c.id.as_str()))
        .map(|c| run_id(c, pick.as_deref()))
        .collect())
}

/// `row` runs: it is no child row, or `admitted` holds its child's schedule id.
pub fn runs(row: &Row, admitted: &BTreeSet<String>) -> bool {
    row.stage != SPAWN_STAGE || admitted.contains(&row.condition)
}

fn unparsed(kind: &str) -> RunError {
    RunError::Records(format!("a {kind} record does not parse"))
}
