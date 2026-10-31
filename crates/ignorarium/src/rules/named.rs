//! The cross-field checks. A rule row names one (`check = "named:<name>"`); its
//! `when` gates it like a generic rule. Each check gets the row's target pattern,
//! selects its nodes with it, and reports only at those nodes. A check skips what
//! is missing or mistyped: the generic rules report that.
use super::generic::{applies, diagnostic, join, join_key, plain_key, safe_key, select};
use super::{BoxRecords, Check, NoRecords, Rule, has_format, is_study_id};
use crate::derived::{
    ANALYSIS, Alpha, Analysis, Carried, Estimate, Instrument, Selection, StudyState, contrast_key,
};
use crate::diag::{Diagnostic, printable};
use crate::inputs::{self, Found, input_path};
use crate::payload;
use crate::power::{required_clusters, z};
use crate::prior::{PriorUse, same_data};
use crate::records;
use crate::runner::PINNED_LEVEL;
use crate::schedule::ROW_CAP;
use crate::seed::seed;
use crate::spec::model::{Claim, Outcome, SELECTED, StatisticSpec};
use crate::store::{Store, is_digest};
use crate::stream::{self, LogName};
use evidence::claim::{Bound, Statistic};
use evidence::power::{Plan, Stratum, WORK, clusters_needed, mean_sd, simulate};
use evidence::rng::Rng;
use evidence::{Error, Tail};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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
    /// The hash the register record and the schedule hold for this spec. It seeds the power
    /// gate, so every check of one spec gives the same power.
    pub spec_hash: [u8; 32],
    /// What the power gate found, so the power report shows the simulation the rules ran.
    pub gates: Gates,
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
            spec_hash: [0; 32],
            gates: Gates::default(),
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
    ("power-at-side", power_at_side),
    ("power-met", power_met),
    ("stream-level", stream_level),
    ("estimates-from-record", estimates_from_record),
    ("reference-finished", reference_finished),
    ("reference-not-self", reference_not_self),
    ("estimates-same-code", estimates_same_code),
    ("deviation-borrowed", deviation_borrowed),
    ("confirmation-disjoint", confirmation_disjoint),
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

fn input_path_safe(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    flag(spec, pat, |v| {
        v.as_str()
            .is_some_and(|rel| input_path(&ctx.root, rel).is_none())
    })
}

/// An input whose path lies inside another input's path (`data/a` and `data/a/b`), or that
/// names an earlier input's file in another spelling (`data/a` and `data//a/`): empty segments
/// are skipped when a path is read. Two equal strings are E-UNQ-001's.
fn input_path_nested(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    let all: Vec<(String, &str, Vec<&str>)> = nodes(spec, pat)
        .into_iter()
        .filter_map(|(p, v)| {
            let raw = v.as_str()?;
            Some((p, raw, raw.split('/').filter(|s| !s.is_empty()).collect()))
        })
        .collect();
    let within = |i: usize| {
        let (_, raw, inner) = &all[i];
        (all.iter().enumerate()).any(|(j, (_, other, outer))| {
            // Shorter, or as long and listed earlier.
            let before = (outer.len(), j) < (inner.len(), i);
            other != raw && !outer.is_empty() && before && inner.starts_with(outer)
        })
    };
    (0..all.len())
        .filter(|&i| within(i))
        .map(|i| hit(all[i].0.clone()))
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
            .and_then(|rel| input_path(&ctx.root, rel))
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

fn open_unit(x: f64) -> bool {
    x > 0.0 && x < 1.0
}

/// Replicates of each power simulation.
const REPS: u32 = 4000;
/// The most clusters per stratum the search for the count that would pass tries.
pub const CAP: u32 = 1024;

/// E-CNF-002, at each hypothesis with a claim that E-CNF-001, E-HYP-003 and E-HYP-004 accept
/// and whose estimates are found: `gate` must find the power the claim needs. An absent power
/// cannot be computed, so it fails closed; a mistyped or out-of-range one, or a units per
/// cluster below 1, is the generic rules', and a stream that gives no level is E-STR-001's.
fn power_met(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    let claimed: Vec<(String, &Value)> = nodes(spec, pat)
        .into_iter()
        .filter(|(_, h)| has_claim(h))
        .collect();
    let design = match gate_design(spec, ctx) {
        Err(Absent) => {
            return claimed
                .into_iter()
                .map(|(path, _)| Hit {
                    path,
                    detail: Some("Cannot compute power: design.power is absent.".into()),
                })
                .collect();
        }
        Ok(None) => return vec![],
        Ok(Some(d)) => d,
    };
    let mut out = vec![];
    for (path, h) in claimed {
        let at = |detail: String| Hit {
            path: path.clone(),
            detail: Some(detail),
        };
        match ctx.gates.of(spec, &path, || gate(spec, ctx, h, design)) {
            None => {}
            Some(Gate::Floor(short)) => out.extend(short.into_iter().map(|(group, have, need)| {
                let group = group_name(&group);
                at(format!(
                    "{}{} has {have} of the {need} clusters the power gate needs.",
                    group[..1].to_ascii_uppercase(),
                    &group[1..]
                ))
            })),
            Some(Gate::Simulated(p)) if p.power >= p.target => {}
            Some(Gate::Simulated(p)) => out.push(at(p.sentence())),
            Some(Gate::Fault(why)) => out.push(at(format!("Cannot simulate power: {why}."))),
        }
    }
    out
}

/// The simulated power of one confirmatory hypothesis, as `check --json` and `review` show it.
#[derive(Debug, Clone, PartialEq)]
pub struct Power {
    /// The hypothesis id; `<name>` when it is not safe to print.
    pub hypothesis: String,
    pub power: f64,
    /// `design.power`.
    pub target: f64,
    /// The clusters of each group the claim is tested in, in the hypothesis's order: the
    /// stratum as `shown` prints it, `""` for the pooled group.
    pub clusters: Vec<(String, usize)>,
    /// When the power falls short: the smallest common count of clusters per group that
    /// reaches the target, or `None` when 1,024 do not. `None` too when the power reaches it.
    pub needed: Option<u32>,
}

impl Power {
    /// `Simulated power 0.62 with 26 clusters in code and 26 in article`, then what would pass.
    pub fn sentence(&self) -> String {
        let groups: Vec<String> = (self.clusters.iter().enumerate())
            .map(|(i, (stratum, n))| {
                let unit = match (i, n) {
                    (0, 1) => " cluster",
                    (0, _) => " clusters",
                    _ => "",
                };
                format!("{n}{unit} in {}", in_group(stratum))
            })
            .collect();
        let groups = match groups.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
            _ => groups.concat(),
        };
        let target = decimals(self.target);
        let pooled = matches!(self.clusters.as_slice(), [(stratum, _)] if stratum.is_empty());
        let per = if pooled { "clusters" } else { "per stratum" };
        let then = match (self.power >= self.target, self.needed) {
            (true, _) => format!("the target is {target}"),
            (false, Some(n)) => format!("about {n} {per} reach {target}"),
            (false, None) => format!("{CAP} {per} do not reach {target}"),
        };
        // Truncated, so a power just short of the target never prints as the target; the
        // 1e-9 absorbs the rounding of k / REPS.
        let power = (self.power * 100.0 + 1e-9).floor() / 100.0;
        format!("Simulated power {power:.2} with {groups}; {then}.")
    }
}

/// A hypothesis id as output may print it: one E-FMT-001 accepts and the secret scan passes,
/// else `<name>`.
fn shown_id(id: &str) -> &str {
    let clean = crate::scan::scan_str(id, crate::scan::builtin()).is_empty();
    if clean && has_format("question.hypotheses[*].id", id) {
        id
    } else {
        "<name>"
    }
}

/// A group as a sentence names it after "in": the stratum, or `the pooled group`.
fn in_group(stratum: &str) -> &str {
    if stratum.is_empty() {
        "the pooled group"
    } else {
        stratum
    }
}

/// `x` with two decimals when they show it exactly, else in full: 0.8 is `0.80`, 0.999 `0.999`.
fn decimals(x: f64) -> String {
    let two = format!("{x:.2}");
    if two.parse::<f64>() == Ok(x) {
        two
    } else {
        x.to_string()
    }
}

/// The simulated power of each confirmatory hypothesis the gate simulates, in order.
pub fn power(spec: &Value, ctx: &CheckCtx) -> Vec<Power> {
    let Ok(Some(design)) = gate_design(spec, ctx) else {
        return vec![];
    };
    if !confirmatory(spec) {
        return vec![];
    }
    nodes(spec, "question.hypotheses[*]")
        .into_iter()
        .filter(|(_, h)| has_claim(h))
        .filter_map(
            |(path, h)| match ctx.gates.of(spec, &path, || gate(spec, ctx, h, design))? {
                Gate::Simulated(p) => Some(p),
                _ => None,
            },
        )
        .collect()
}

/// `design.power` is absent.
struct Absent;

/// The level the stream of the plan's log gives it, `design.power` and
/// `design.units_per_cluster`, when the gate can compute with them. `Ok(None)`: one is
/// mistyped or out of range, which the generic rules report, or the stream gives no level or
/// cannot be read, which E-STR-001 reports from the same read.
fn gate_design(spec: &Value, ctx: &CheckCtx) -> Result<Option<(f64, f64, u64)>, Absent> {
    let power = val(spec, "design.power").ok_or(Absent)?.as_f64();
    let per = val(spec, "design.units_per_cluster").and_then(Value::as_u64);
    let level = (granted(spec, ctx).ok().flatten())
        .map(|a| a.level)
        .filter(|l| *l > 0.0);
    Ok(match (level, power, per) {
        (Some(level), Some(power), Some(per)) if per >= 1 && open_unit(power) => {
            Some((level, power, per))
        }
        _ => None,
    })
}

/// The log a plan registers into, by `study.test_log`; `None` when that is not a bool.
fn stream_log(spec: &Value) -> Option<&'static str> {
    let test_log = val(spec, "study.test_log")?.as_bool()?;
    Some(LogName::of(test_log).as_str())
}

/// The slot and level the stream of a confirmatory plan's log would give it now. `Ok(None)`
/// for an exploratory plan or a `study.test_log` that is not a bool; `Err` when the stream
/// cannot be read or trusted.
pub fn granted(spec: &Value, ctx: &CheckCtx) -> Result<Option<Alpha>, String> {
    let Some(log) = stream_log(spec).filter(|_| confirmatory(spec)) else {
        return Ok(None);
    };
    let next = ctx.records.stream_next(log)?;
    Ok(Some(Alpha {
        log: log.into(),
        slot: next.slot,
        level: next.level,
    }))
}

/// E-STR-001 at `study.mode` of a confirmatory plan: the stream of its log gives level 0 (a
/// slot beyond the plan stream's last), or cannot be read or trusted.
fn stream_level(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    let detail = match granted(spec, ctx) {
        Ok(None) => return vec![],
        Ok(Some(a)) if a.level > 0.0 => return vec![],
        Ok(Some(a)) => format!(
            "Slot {} is beyond the {} slots of the {} stream.",
            a.slot,
            stream::M,
            a.log
        ),
        Err(e) => format!(
            "The {} stream cannot be read: {e}.",
            stream_log(spec).unwrap_or_default()
        ),
    };
    nodes(spec, pat)
        .into_iter()
        .map(|(path, _)| Hit {
            path,
            detail: Some(detail.clone()),
        })
        .collect()
}

/// What the power gate finds for one hypothesis.
#[derive(Clone)]
enum Gate {
    /// The z floor fails in these groups: (stratum, clusters, clusters a z-test needs).
    Floor(Vec<(String, u64, u64)>),
    Simulated(Power),
    /// The simulation refused its plan; the text says why.
    Fault(String),
}

/// The gate's findings by hypothesis path, for the one spec they came from. A finding is
/// computed once per spec and path; another spec starts the memo afresh.
#[derive(Default)]
pub struct Gates(RefCell<(Value, BTreeMap<String, Option<Gate>>)>);

impl Gates {
    /// The finding at `path` of `spec`, from `find` the first time it is asked for.
    fn of(&self, spec: &Value, path: &str, find: impl FnOnce() -> Option<Gate>) -> Option<Gate> {
        let found = {
            let mut memo = self.0.borrow_mut();
            if memo.0 != *spec {
                *memo = (spec.clone(), BTreeMap::new());
            }
            memo.1.get(path).cloned()
        };
        found.unwrap_or_else(|| {
            let g = find();
            self.0.borrow_mut().1.insert(path.into(), g.clone());
            g
        })
    }
}

/// The power gate of hypothesis `h`. It simulates the test the box runs (`evidence::claim`) at
/// the stream's level x `alpha_share`. In each group the claim is tested in, units have the
/// sd and icc of the group's estimate around a mean at `power_at` (a mean claim), or at
/// `power_at` - sd z(q), which puts the units' q-quantile at `power_at` (a quantile claim), over
/// the group's own clusters. The seed is `power/<id>` under the spec hash. A mean claim first
/// meets the one-sided z count for every bound: the box estimates the statistic as the
/// unweighted mean of the cluster means, so a cluster mean has sd `sd x sqrt(icc + (1 - icc) /
/// m)` on average, m the harmonic mean of the units the group's clusters give. A shortfall
/// there fails without simulating. `None` when a rule before it reports the hypothesis, or a
/// group has no cluster (E-HYP-002).
fn gate(spec: &Value, ctx: &CheckCtx, h: &Value, design: (f64, f64, u64)) -> Option<Gate> {
    let (stream_level, target, per) = design;
    let sized = sizing(h)?;
    let Some(Lookup::Found(groups)) = estimates_of(spec, h, ctx) else {
        return None;
    };
    let level = stream_level * sized.share;
    let groups: Vec<(StratumInput, Vec<u32>)> = (groups.into_iter())
        .map(|g| {
            let sizes = (cluster_sizes(spec, ctx, &g.stratum, per).into_iter())
                .map(|u| u32::try_from(u).unwrap_or(u32::MAX))
                .collect();
            (g, sizes)
        })
        .collect();
    if let Statistic::Mean { .. } = sized.stat {
        let short: Vec<(String, u64, u64)> = (groups.iter())
            .filter(|(_, sizes)| !sizes.is_empty())
            .filter_map(|(g, sizes)| {
                let have = sizes.len() as u64;
                let sd = mean_sd(g.sd, g.icc, sizes);
                let need = (sized.bounds.iter())
                    .map(|b| required_clusters(level, target, sd, (b.margin - sized.at).abs()))
                    .max()
                    .unwrap_or(0);
                (have < need).then(|| (g.stratum.clone(), have, need))
            })
            .collect();
        if !short.is_empty() {
            return Some(Gate::Floor(short));
        }
    }
    if groups.iter().any(|(_, sizes)| sizes.is_empty()) {
        return None;
    }
    let strata = (groups.iter())
        .map(|(g, sizes)| Stratum {
            sizes: sizes.clone(),
            sd: g.sd,
            icc: g.icc,
            unit_mean: match sized.stat {
                Statistic::Mean { .. } => sized.at,
                Statistic::Quantile(q) => sized.at - g.sd * z(q),
            },
        })
        .collect();
    let plan = Plan {
        stat: sized.stat,
        bounds: sized.bounds,
        strata,
        level,
    };
    let id = h.get("id").and_then(Value::as_str).unwrap_or_default();
    let seed = seed(&ctx.spec_hash, &format!("power/{id}"));
    let found = simulate(&plan, REPS, &mut Rng::from_seed(seed)).and_then(|power| {
        let needed = if power < target {
            clusters_needed(&plan, target, REPS, seed, CAP)?
        } else {
            None
        };
        Ok((power, needed))
    });
    Some(match found {
        Err(Error::Parameter("work")) => Gate::Fault(format!(
            "it would draw more than {} million values",
            WORK / 1_000_000
        )),
        Err(e) => Gate::Fault(e.to_string()),
        Ok((power, needed)) => Gate::Simulated(Power {
            hypothesis: shown_id(id).to_string(),
            power,
            target,
            clusters: (groups.iter())
                .map(|(g, sizes)| {
                    let pooled = g.stratum.is_empty();
                    let name = if pooled { "" } else { shown(&g.stratum) };
                    (name.to_string(), sizes.len())
                })
                .collect(),
            needed,
        }),
    })
}

/// What the power gate sizes a hypothesis at.
struct Sizing {
    stat: Statistic,
    bounds: Vec<Bound>,
    at: f64,
    share: f64,
}

/// `None` unless the claim and statistic are well formed with every margin inside the mean
/// range (E-HYP-003), `power_at` passes E-CNF-001 and `alpha_share` passes E-HYP-004.
fn sizing(h: &Value) -> Option<Sizing> {
    let claim = claim_of(h)?;
    let stat = statistic_of(h)?;
    let bounds = claim.bounds();
    let margins: Vec<f64> = bounds.iter().map(|b| b.margin).collect();
    if !inside_range(h, &margins) {
        return None;
    }
    let at = h
        .get("power_at")?
        .as_f64()
        .filter(|p| sized_at(h, &claim, *p))?;
    let share = h
        .get("alpha_share")?
        .as_f64()
        .filter(|x| share_in_range(*x))?;
    Some(Sizing {
        stat,
        bounds,
        at,
        share,
    })
}

/// The statistic of a hypothesis, when it is well formed.
fn statistic_of(h: &Value) -> Option<Statistic> {
    let statistic = h.get("statistic")?;
    mean_range(statistic)?.ok()?;
    let parsed: StatisticSpec = serde_json::from_value(statistic.clone()).ok()?;
    Some(parsed.to_evidence())
}

/// Every margin lies strictly inside the statistic's mean range, when it has one.
fn inside_range(h: &Value, margins: &[f64]) -> bool {
    match h.get("statistic").and_then(mean_range) {
        Some(Ok(Some((lo, hi)))) => margins.iter().all(|&m| lo < m && m < hi),
        _ => true,
    }
}

/// The units each confirmation cluster of `stratum` gives (every confirmation input for the
/// pooled group `""`): its declared units, at most `per`. As in `analysis::distinct_clusters`,
/// an input that holds the same data as an earlier confirmation input, in any stratum, is no
/// cluster; an input whose data cannot be read counts on its own. An input with no units is
/// not counted: E-UNT-001 reports it.
fn cluster_sizes(spec: &Value, ctx: &CheckCtx, stratum: &str, per: u64) -> Vec<u64> {
    let mut seen: Vec<(String, String)> = vec![];
    let mut sizes = vec![];
    for (_, input) in nodes(spec, "inputs[*]") {
        if role(input) != Some("confirmation") {
            continue;
        }
        let Some(units) = input
            .get("units")
            .and_then(Value::as_u64)
            .filter(|u| *u >= 1)
        else {
            continue;
        };
        if let Some(h) = input_hashes(ctx, input) {
            if seen.iter().any(|s| same_data(s, &h)) {
                continue;
            }
            seen.push(h);
        }
        let in_group =
            stratum.is_empty() || input.get("stratum").and_then(Value::as_str) == Some(stratum);
        if in_group {
            sizes.push(units.min(per));
        }
    }
    sizes
}

/// The claim of a hypothesis, when it is well formed.
fn claim_of(h: &Value) -> Option<Claim> {
    let claim = h.get("claim")?;
    margins(claim)?.ok()?;
    serde_json::from_value(claim.clone()).ok()
}

/// `p` lies strictly on the claimed side of every bound, and inside the statistic's range of
/// a cluster mean when it has one.
fn sized_at(h: &Value, claim: &Claim, p: f64) -> bool {
    let side = claim.bounds().iter().all(|b| match b.tail {
        Tail::Lower => p < b.margin,
        Tail::Upper => p > b.margin,
        Tail::Both => false,
    });
    let inside = match h.get("statistic").and_then(mean_range) {
        Some(Ok(Some((lo, hi)))) => (lo..=hi).contains(&p),
        _ => true,
    };
    side && inside
}

/// E-CNF-001, at `<h>.power_at` of a hypothesis with a well-formed claim: absent, or not
/// where `sized_at` puts it. A mistyped value is the type rule's.
fn power_at_side(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    children(spec, pat)
        .into_iter()
        .filter(|(_, h, v)| {
            claim_of(h).is_some_and(|claim| match v {
                None => true,
                Some(v) => v.as_f64().is_some_and(|p| !sized_at(h, &claim, p)),
            })
        })
        .map(|(p, _, _)| hit(p))
        .collect()
}

// ---- values from earlier studies ----

/// What a confirmatory hypothesis takes from the study its `estimates` names.
#[derive(Debug, Clone, PartialEq)]
pub enum Lookup {
    /// The study has not finished; the text says which way.
    Pending(String),
    /// No usable estimate; the text says why.
    Missing(String),
    /// One per group the hypothesis is tested in.
    Found(Vec<StratumInput>),
}

/// The sd and icc the power gate uses in one group: the largest over the contrasts the
/// hypothesis stands for, and the hashes of the estimate records they come from. A negative
/// icc, which an analysis of variance can give, counts as 0, the larger and so safer value.
#[derive(Debug, Clone, PartialEq)]
pub struct StratumInput {
    /// `""` for the pooled group.
    pub stratum: String,
    pub sd: f64,
    pub icc: f64,
    /// The sd and icc come from the pooled estimate, because the stratum's own has no usable icc
    /// or the study has none for the stratum.
    pub pooled: bool,
    pub records: Vec<String>,
}

/// The sd and icc of hypothesis `h` (its index in `question.hypotheses`), from the study its
/// `estimates` names. That study must have finished. For each stratum the hypothesis lists
/// (the pooled group `""` when none) and each contrast it stands for (with `selected`, each
/// candidate of this plan's selection stage, with its params), the estimate with that
/// contrast's key and the stratum, else the pooled one. An estimate without an icc, or with
/// no variance above 0, is skipped. Several such estimates must agree. The largest sd and icc over the candidates.
pub fn estimate_inputs(spec: &Value, h: usize, ctx: &CheckCtx) -> Lookup {
    estimates_of(spec, &spec["question"]["hypotheses"][h], ctx)
        .unwrap_or_else(|| Lookup::Missing("The hypothesis is not well formed.".into()))
}

/// What a condition param takes from the study it names.
#[derive(Debug, Clone, PartialEq)]
pub enum Taken {
    /// The study has not finished; the text says which way.
    Pending(String),
    /// The study cannot give a condition; the text says why.
    Refused(String),
    /// The condition the study selected, and the hash of the `selection` record it comes from.
    Found { selected: String, record: String },
}

/// The condition that study `from` selected, from its `selection` record. The study must have
/// finished.
pub fn selected_by(ctx: &CheckCtx, from: &str) -> Taken {
    let read = ctx.records.study(from).and_then(|all| {
        let state = StudyState::of(all.clone(), |log, slot| {
            ctx.records.stream_closed(log, from, slot)
        })?;
        Ok((all, state))
    });
    let (all, state) = match read {
        Err(e) => {
            return Taken::Refused(format!(
                "The box cannot read the records of study {from}: {e}."
            ));
        }
        Ok(read) => read,
    };
    match state {
        StudyState::NotRegistered => Taken::Pending(waits(from, "not registered")),
        StudyState::NotFinished => Taken::Pending(waits(from, "not finished")),
        StudyState::Finished(_) => {
            // The selection record precedes the analysis record, so it is not among the
            // records `Finished` holds.
            let found = all.into_iter().flatten().find_map(|r| {
                let s = Selection::of(&r)?;
                Some((s.selected, records::hash(&r)))
            });
            match found {
                Some((selected, record)) => Taken::Found { selected, record },
                None => Taken::Refused(format!("Study {from} recorded no selection.")),
            }
        }
    }
}

/// A contrast a hypothesis stands for: the candidate it takes as the pick, and its key hash.
struct Concrete<'a> {
    pick: Option<&'a str>,
    key: String,
}

/// `estimate_inputs`, with `None` when a generic, shape or stage rule reports the hypothesis or
/// its reference, or a param reference leaves the contrast's key unknown, so the rules that read
/// estimates skip it. A study that has not finished is pending whatever the key.
fn estimates_of(spec: &Value, hyp: &Value, ctx: &CheckCtx) -> Option<Lookup> {
    let Some(reference) = hyp.get("estimates") else {
        return Some(Lookup::Missing("The hypothesis names no study.".into()));
    };
    let from = study_ref(spec, reference)?;
    let state = ctx.records.study(from).and_then(|all| {
        StudyState::of(all, |log, slot| ctx.records.stream_closed(log, from, slot))
    });
    let records = match state {
        Err(e) => {
            return Some(Lookup::Missing(format!(
                "The box cannot read the records of study {from}: {e}."
            )));
        }
        Ok(StudyState::NotRegistered) => {
            return Some(Lookup::Pending(waits(from, "not registered")));
        }
        Ok(StudyState::NotFinished) => return Some(Lookup::Pending(waits(from, "not finished"))),
        Ok(StudyState::Finished(records)) => records,
    };
    let contrasts = contrasts(spec, hyp, ctx)?;
    let groups = groups_of(hyp)?;
    let estimates: Vec<(String, Estimate)> = records
        .iter()
        .filter_map(|r| Some((records::hash(r), Estimate::of(r)?)))
        .collect();
    let mut out = Vec::new();
    for stratum in groups {
        let mut input = StratumInput {
            stratum: stratum.into(),
            sd: 0.0,
            icc: 0.0,
            pooled: false,
            records: vec![],
        };
        for c in &contrasts {
            let pick = c
                .pick
                .map(|p| format!(" with {} as the pick", shown(p)))
                .unwrap_or_default();
            let matching = |group: &str| -> Vec<&(String, Estimate)> {
                (estimates.iter())
                    .filter(|(_, e)| e.key_hash == c.key && e.stratum == group)
                    .collect()
            };
            // The first group with an estimate that has an icc and a variance above 0.
            let found = [stratum, ""].into_iter().find_map(|group| {
                let usable: Vec<_> = (matching(group).into_iter())
                    .filter(|(_, e)| e.icc.is_some() && e.var > 0.0)
                    .collect();
                (!usable.is_empty()).then_some((group, usable))
            });
            let Some((group, all)) = found else {
                if let Some(group) = [stratum, ""].into_iter().find(|g| !matching(g).is_empty()) {
                    return Some(Lookup::Missing(format!(
                        "Study {from}'s estimate of this contrast{pick} in {} has no icc, or no variance above 0.",
                        group_name(group)
                    )));
                }
                let wanted = if stratum.is_empty() {
                    group_name("")
                } else {
                    format!("{} or the pooled group", group_name(stratum))
                };
                return Some(Lookup::Missing(format!(
                    "Study {from} has no estimate of this contrast{pick} in {wanted}."
                )));
            };
            input.pooled |= group != stratum;
            let e = &all[0].1;
            if all.iter().any(|(_, x)| x.var != e.var || x.icc != e.icc) {
                return Some(Lookup::Missing(format!(
                    "Study {from} holds {} estimates of this contrast{pick} in {}, with different variances or iccs.",
                    all.len(),
                    group_name(group)
                )));
            }
            let icc = e.icc.unwrap_or_default();
            input.sd = input.sd.max(e.var.sqrt());
            input.icc = input.icc.max(icc);
            input
                .records
                .extend(all.iter().map(|(hash, _)| hash.clone()));
        }
        out.push(input);
    }
    Some(Lookup::Found(out))
}

fn waits(study: &str, state: &str) -> String {
    format!("Waits on study {study}, which is {state}.")
}

/// A group as a diagnostic names it: `stratum <name>`, or `the pooled group`.
fn group_name(stratum: &str) -> String {
    if stratum.is_empty() {
        "the pooled group".into()
    } else {
        format!("stratum {}", shown(stratum))
    }
}

/// A name read from the spec, as a diagnostic may print it.
fn shown(name: &str) -> &str {
    if safe_key(name) { name } else { "<name>" }
}

/// The study an `estimates` table names. `None` when it is not a table, its `from` is not a
/// study id, or it names this study: the generic rules and E-REF-002 report those.
fn study_ref<'a>(spec: &Value, reference: &'a Value) -> Option<&'a str> {
    let from = reference.get("from")?.as_str()?;
    (is_study_id(from) && Some(from) != text(spec, "study.id")).then_some(from)
}

/// The groups a hypothesis is tested in: each stratum it lists, or the pooled group `""`.
fn groups_of(hyp: &Value) -> Option<Vec<&str>> {
    let Some(strata) = hyp.get("strata") else {
        return Some(vec![""]);
    };
    let names: Vec<&str> = strata
        .as_array()?
        .iter()
        .map(Value::as_str)
        .collect::<Option<_>>()?;
    Some(if names.is_empty() { vec![""] } else { names })
}

/// The contrasts a hypothesis stands for, each with its key, against the plan's reference
/// with its params: one, or with `selected` one per candidate of the selection stage. `None`
/// when the reference, its outcome, a weight, a term or a candidate is one that E-REF-001,
/// `hypothesis-shape`, `condition-declared` or `param-shape` refuses.
fn contrasts<'a>(spec: &'a Value, hyp: &'a Value, ctx: &CheckCtx) -> Option<Vec<Concrete<'a>>> {
    let id = hyp.get("outcome")?.as_str()?;
    let outcome = nodes(spec, "outcomes[*]")
        .into_iter()
        .find(|(_, o)| o.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|(_, o)| serde_json::from_value::<Outcome>(o.clone()).ok())?;
    let reference = text(spec, "design.reference")?;
    let reference = (reference.to_string(), params_of(spec, ctx, reference)?);
    let repeats = match val(spec, "design.repeats") {
        None => 1,
        Some(r) => r.as_u64()?,
    };
    let terms_ok = names_for(spec, CONTRAST_KEYS)?;
    let terms: Vec<(&str, f64)> = hyp
        .get("contrast")?
        .as_object()?
        .iter()
        .map(|(k, w)| {
            let w = w.as_f64().filter(|w| *w != 0.0)?;
            terms_ok.may_name(k).then_some((k.as_str(), w))
        })
        .collect::<Option<_>>()?;
    if terms.is_empty() {
        return None;
    }
    let picks: Vec<Option<&str>> = if terms.iter().any(|(k, _)| *k == SELECTED) {
        let candidates_ok = names_for(spec, SELECTION_LIST)?;
        let list = list(spec, "design.selection.conditions")?;
        let candidates: Vec<&str> = list
            .iter()
            .map(|c| c.as_str().filter(|c| candidates_ok.may_name(c)))
            .collect::<Option<_>>()?;
        if candidates.is_empty() {
            return None;
        }
        candidates.into_iter().map(Some).collect()
    } else {
        vec![None]
    };
    picks
        .into_iter()
        .map(|pick| {
            let concrete = terms
                .iter()
                .map(|&(k, w)| {
                    let c = if k == SELECTED { pick? } else { k };
                    Some((c.to_string(), w, params_of(spec, ctx, c)?))
                })
                .collect::<Option<Vec<_>>>()?;
            let key = contrast_key(&outcome, repeats, &reference, &concrete).1;
            Some(Concrete { pick, key })
        })
        .collect()
}

/// The params of declared condition `id`: its `params` table with each reference replaced by the
/// condition its study selected, `{}` when it has none. `None` when no condition has the id, a
/// param is malformed, or a reference is pending or refused.
fn params_of(spec: &Value, ctx: &CheckCtx, id: &str) -> Option<Value> {
    let (_, condition) = nodes(spec, "design.conditions[*]")
        .into_iter()
        .find(|(_, c)| c.get("id").and_then(Value::as_str) == Some(id))?;
    let Some(params) = condition.get("params") else {
        return Some(json!({}));
    };
    let taken = params.as_object()?.iter().map(|(k, v)| {
        let value = match reference_of(v) {
            Some(from) => match selected_by(ctx, from) {
                Taken::Found { selected, .. } => json!(selected),
                _ => return None,
            },
            None => is_scalar(v).then(|| v.clone())?,
        };
        Some((k.clone(), value))
    });
    Some(Value::Object(taken.collect::<Option<_>>()?))
}

/// E-CNF-003, at `<h>.estimates` of a hypothesis with a claim: absent, or no usable estimate
/// in the finished study it names.
fn estimates_from_record(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    looked_up(spec, ctx, pat, |l| match l {
        Lookup::Missing(why) => Some(why),
        _ => None,
    })
}

/// W-REF-001, at `<h>.estimates` of a hypothesis with a claim: the study it names has not
/// finished.
fn reference_finished(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    if pat.ends_with(".estimates") {
        return looked_up(spec, ctx, pat, |l| match l {
            Lookup::Pending(why) => Some(why),
            _ => None,
        });
    }
    (references(spec, pat).into_iter())
        .filter_map(|(path, from)| match selected_by(ctx, from) {
            Taken::Pending(why) => Some(Hit {
                path,
                detail: Some(why),
            }),
            _ => None,
        })
        .collect()
}

/// The params of the tables `pat` selects that name another study, as (path, study): the key
/// must be one a diagnostic can print, and the study is not this one.
fn references<'a>(spec: &'a Value, pat: &str) -> Vec<(String, &'a str)> {
    let own = text(spec, "study.id");
    nodes(spec, pat.strip_suffix(".*").unwrap_or(pat))
        .into_iter()
        .filter_map(|(path, t)| Some((path, t.as_object()?)))
        .flat_map(|(path, t)| {
            (t.iter())
                .filter(|(k, _)| safe_key(k))
                .filter_map(move |(k, v)| Some((join(&path, k), reference_of(v)?)))
        })
        .filter(|(_, from)| is_study_id(from) && Some(*from) != own)
        .collect()
}

/// A hit with `pick`'s detail at each `<h>.estimates` of a hypothesis with a claim, in a
/// confirmatory study.
fn looked_up(
    spec: &Value,
    ctx: &CheckCtx,
    pat: &str,
    pick: impl Fn(Lookup) -> Option<String>,
) -> Vec<Hit> {
    if !confirmatory(spec) {
        return vec![];
    }
    children(spec, pat)
        .into_iter()
        .filter(|(_, h, _)| has_claim(h))
        .filter_map(|(path, h, _)| {
            Some(Hit {
                path,
                detail: Some(pick(estimates_of(spec, h, ctx)?)?),
            })
        })
        .collect()
}

fn confirmatory(spec: &Value) -> bool {
    text(spec, "study.mode") == Some("confirmatory")
}

// ---- the code of the studies estimates come from ----

/// A finished study a confirmatory hypothesis with a claim takes estimates from.
#[derive(Debug, Clone, PartialEq)]
pub struct Borrowed {
    pub study: String,
    /// The hash of the study's `analysis` record, which names its code.
    pub analysis: String,
    pub instrument: Instrument,
    /// The study's `analysis` record holds a determinism check that passed.
    pub recheck_passed: bool,
    /// The parts of the study's code this plan does not pin: a subset of `payload`,
    /// `environment`, `inputs`, in that order.
    pub differs: Vec<&'static str>,
}

/// The passed determinism check a plan that declares `pins.deterministic` takes instead of
/// running its own: that of the first study in `borrowed` whose check passed and whose payload
/// and environment the plan pins. The pins name what the study's rows ran, so the plan's rows
/// must run in its pinned environment too: `scope.min_isolation` is `PINNED_LEVEL` or more,
/// since a host without that level runs a plan at `min_isolation = 0` on its own root. Other
/// model or calibration inputs do not bar a carry: they are data the code reads, not the code.
pub fn carried(spec: &Value, ctx: &CheckCtx) -> Option<Carried> {
    let declared = val(spec, "pins.deterministic") == Some(&Value::Bool(true));
    let level = val(spec, "scope.min_isolation").and_then(Value::as_u64);
    if !declared || level.is_none_or(|l| l < u64::from(PINNED_LEVEL)) {
        return None;
    }
    (borrowed(spec, ctx).into_iter())
        .find(|b| b.recheck_passed && b.differs.iter().all(|p| *p == "inputs"))
        .map(|b| Carried {
            study: b.study,
            record: b.analysis,
        })
}

/// The studies the confirmatory hypotheses with a claim take estimates from, each once, first
/// mention first: the ones `register` freezes values from. A reference that is not a study id,
/// or names this study, names none.
fn sources(spec: &Value) -> Vec<&str> {
    let mut out = vec![];
    if !confirmatory(spec) {
        return out;
    }
    for (_, h) in nodes(spec, "question.hypotheses[*]") {
        let reference = h.get("estimates").filter(|_| has_claim(h));
        if let Some(from) = reference.and_then(|r| study_ref(spec, r))
            && !out.contains(&from)
        {
            out.push(from);
        }
    }
    out
}

/// One per study in `sources` that has finished, with the parts of its code this plan does
/// not pin. A study whose records cannot be read is E-CNF-003's.
pub fn borrowed(spec: &Value, ctx: &CheckCtx) -> Vec<Borrowed> {
    let finished: Vec<(&str, String, Analysis)> = (sources(spec).into_iter())
        .filter_map(|from| {
            let all = ctx.records.study(from).ok()??;
            let record = all.iter().find(|r| r.kind == ANALYSIS)?;
            let hash = records::hash(record);
            let analysis = Analysis::of(record)?;
            let state = StudyState::of(Some(all), |log, slot| {
                ctx.records.stream_closed(log, from, slot)
            });
            matches!(state, Ok(StudyState::Finished(_))).then_some((from, hash, analysis))
        })
        .collect();
    if finished.is_empty() {
        return vec![];
    }
    let pinned = Pinned::of(spec, &ctx.root);
    (finished.into_iter())
        .map(|(study, hash, a)| Borrowed {
            study: study.into(),
            analysis: hash,
            differs: pinned.differs(&a.instrument),
            instrument: a.instrument,
            recheck_passed: a.recheck.is_some_and(|r| r.passed()),
        })
        .collect()
}

/// The code a plan pins: its payload, its environment, and its model and calibration inputs
/// as (path, role, pin).
struct Pinned {
    payload: String,
    environment: String,
    inputs: BTreeSet<(String, String, String)>,
}

impl Pinned {
    /// An empty `pins.payload` stands for the hash of the payload folder under `root`, which
    /// `register` freezes; a folder that cannot be hashed pins nothing.
    fn of(spec: &Value, root: &Path) -> Pinned {
        let pin = |path: &str| {
            let p = text(spec, path).unwrap_or_default().trim();
            p.strip_prefix("sha256:").unwrap_or(p).to_string()
        };
        let mut payload = pin("pins.payload");
        if payload.is_empty() {
            payload = payload::tree_hash(&root.join(payload::DIR)).unwrap_or_default();
        }
        let field = |i: &Value, k: &str| {
            let v = i.get(k).and_then(Value::as_str);
            v.unwrap_or_default().to_string()
        };
        let inputs = (nodes(spec, "inputs[*]").into_iter())
            .filter(|(_, i)| matches!(role(i), Some("model" | "calibration")))
            .map(|(_, i)| (field(i, "path"), field(i, "role"), field(i, "sha256")))
            .collect();
        Pinned {
            payload,
            environment: pin("pins.environment"),
            inputs,
        }
    }

    /// The parts of `theirs` this plan does not pin. A part is pinned by digests equal to the
    /// study's, so an empty pin pins nothing, and neither does a study's `host` or `""`
    /// environment: neither names one image.
    fn differs(&self, theirs: &Instrument) -> Vec<&'static str> {
        let same = |ours: &str, theirs: &str| is_digest(ours) && ours == theirs;
        let inputs = self.inputs.iter().all(|(_, _, pin)| is_digest(pin))
            && self.inputs == theirs.inputs.iter().cloned().collect();
        [
            ("payload", same(&self.payload, &theirs.payload)),
            ("environment", same(&self.environment, &theirs.environment)),
            ("inputs", inputs),
        ]
        .into_iter()
        .filter_map(|(part, pinned)| (!pinned).then_some(part))
        .collect()
    }
}

/// `Study <s> ran payload <hash>, environment <digest>; this plan differs in <parts>.`, with
/// the study's model and calibration inputs after the environment when they differ, so the
/// plan can pin them. A value the study recorded as `""` prints as `none`.
fn ran(b: &Borrowed) -> String {
    let shown = |x: &str| if x.is_empty() { "none" } else { x }.to_string();
    let listed: Vec<String> = (b.instrument.inputs.iter())
        .map(|(path, role, sha)| format!("{} ({role}, {})", printable(path), shown(sha)))
        .collect();
    let inputs = match (b.differs.contains(&"inputs"), listed.is_empty()) {
        (false, _) => String::new(),
        (true, true) => ", and no model or calibration input".into(),
        (true, false) => format!(", and model and calibration inputs {}", listed.join(", ")),
    };
    format!(
        "Study {} ran payload {}, environment {}{inputs}; this plan differs in {}.",
        b.study,
        shown(&b.instrument.payload),
        shown(&b.instrument.environment),
        b.differs.join(", ")
    )
}

/// E-REF-003, at `<h>.estimates` of a hypothesis with a claim in a confirmatory study: the
/// finished study it names ran code this plan does not pin, and no deviation names that
/// study.
fn estimates_same_code(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    let found = borrowed(spec, ctx);
    let declared = texts(spec, "pins.deviations[*].from");
    (children(spec, pat).into_iter())
        .filter(|(_, h, _)| has_claim(h))
        .filter_map(|(path, _, reference)| {
            let from = study_ref(spec, reference?)?;
            let b = (found.iter()).find(|b| b.study == from && !b.differs.is_empty())?;
            (!declared.contains(&from)).then(|| Hit {
                path,
                detail: Some(ran(b)),
            })
        })
        .collect()
}

/// E-REF-003, at a deviation's `from`: a study id that no confirmatory hypothesis with a claim
/// takes estimates from. An exploratory study takes none, so it declares no deviation, and a
/// recorded deviation always means the box compared the code. A `from` that is not a study id
/// is the generic rules'.
fn deviation_borrowed(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    let sources = sources(spec);
    (nodes(spec, pat).into_iter())
        .filter_map(|(path, v)| {
            let from = v
                .as_str()
                .filter(|f| is_study_id(f) && !sources.contains(f))?;
            Some(Hit {
                path,
                detail: Some(format!(
                    "Only a confirmatory hypothesis with a claim takes estimates, and none takes them from study {from}."
                )),
            })
        })
        .collect()
}

/// E-REF-002. At `<h>.estimates.from` of a confirmatory study: a reference names this study,
/// whose values do not exist until it runs. At a param's `.from`: the same, or the study it names
/// has finished and holds no selection.
fn reference_not_self(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    let own = text(spec, "study.id");
    let names_self = |v: &Value| v.as_str().is_some_and(|from| Some(from) == own);
    if pat.ends_with(".estimates.from") {
        return if confirmatory(spec) {
            flag(spec, pat, names_self)
        } else {
            vec![]
        };
    }
    let table = pat.strip_suffix(".from").unwrap_or(pat);
    let mut hits = flag(spec, pat, names_self);
    hits.extend(
        references(spec, table)
            .into_iter()
            .filter_map(|(path, from)| match selected_by(ctx, from) {
                Taken::Refused(why) => Some(Hit {
                    path: join(&path, "from"),
                    detail: Some(why),
                }),
                _ => None,
            }),
    );
    hits
}

/// At `inputs`: no confirmation input. At `inputs[*].<field>`: a confirmation input holds the
/// data of a selection or calibration input: the same exact hash, or the same normalized hash
/// (text that differs only in case and spacing). An empty normalized hash (not UTF-8 text)
/// matches nothing. The detail names the first such input in spec order.
fn confirmation_disjoint(spec: &Value, ctx: &CheckCtx, pat: &str) -> Vec<Hit> {
    if !pat.contains('.') {
        return flag(spec, pat, |v| {
            v.as_array()
                .is_some_and(|a| !a.iter().any(|i| role(i) == Some("confirmation")))
        });
    }
    let held: Vec<(String, &str, (String, String))> = nodes(spec, "inputs[*]")
        .into_iter()
        .filter_map(|(p, i)| match role(i)? {
            r @ ("selection" | "calibration") => Some((p, r, input_hashes(ctx, i)?)),
            _ => None,
        })
        .collect();
    with_parent(spec, pat)
        .into_iter()
        .filter(|(_, input, _)| role(input) == Some("confirmation"))
        .filter_map(|(path, input, _)| {
            let h = input_hashes(ctx, input)?;
            let (other, r, _) = held.iter().find(|(_, _, s)| same_data(s, &h))?;
            Some(Hit {
                path,
                detail: Some(format!("It holds the data of {other}, a {r} input.")),
            })
        })
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
const SELECTION_LIST: &str = "design.selection.conditions[*]";

/// The condition names a place may use, and the reference, which it may not.
struct Names<'a> {
    usable: Vec<&'a str>,
    reference: Option<&'a str>,
}

impl Names<'_> {
    fn may_name(&self, c: &str) -> bool {
        self.usable.contains(&c) && Some(c) != self.reference
    }
}

/// The names the place `pat` may use (see `condition_declared`). `None` when there is nothing
/// to check against: no confirmation list for a contrast key, no declared condition id for
/// the others.
fn names_for<'a>(spec: &'a Value, pat: &str) -> Option<Names<'a>> {
    let staged = val(spec, "design.selection").is_some();
    let usable: Vec<&str> = if pat == CONTRAST_KEYS {
        list(spec, "design.confirmation.conditions")?;
        let runs = texts(spec, "design.confirmation.conditions[*]");
        runs.into_iter()
            .filter(|c| *c != SELECTED || staged)
            .collect()
    } else {
        let declared = texts(spec, "design.conditions[*].id");
        if declared.is_empty() {
            return None;
        }
        let pick = staged && pat == "design.confirmation.conditions[*]";
        declared
            .into_iter()
            .chain(pick.then_some(SELECTED))
            .collect()
    };
    let reference = text(spec, "design.reference").filter(|_| pat != "design.reference");
    Some(Names { usable, reference })
}

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
    let Some(names) = names_for(spec, pat) else {
        return vec![];
    };
    if pat == CONTRAST_KEYS {
        return keys(spec, pat)
            .into_iter()
            .filter(|(_, k)| !names.may_name(k))
            .map(|(p, _)| hit(p))
            .collect();
    }
    flag(spec, pat, |v| {
        v.as_str().is_some_and(|c| !names.may_name(c))
    })
}

/// A param value is a string or a number.
fn is_scalar(v: &Value) -> bool {
    v.is_string() || v.is_number()
}

/// The study a param names: `{ from = "<study id>" }`, and nothing else in the table.
fn reference_of(v: &Value) -> Option<&str> {
    let m = v.as_object()?;
    (m.len() == 1).then(|| m.get("from")?.as_str())?
}

/// A param is a scalar, or a reference to a study. A reference needs a key a diagnostic can print:
/// `register` freezes it under that key.
fn param_shape(spec: &Value, _: &CheckCtx, pat: &str) -> Vec<Hit> {
    let tables = nodes(spec, pat.strip_suffix(".*").unwrap_or(pat));
    tables
        .into_iter()
        .filter_map(|(path, t)| Some((path, t.as_object()?)))
        .flat_map(|(path, t)| {
            t.iter()
                .filter(|(k, v)| {
                    if v.is_object() {
                        !(reference_of(v).is_some() && safe_key(k))
                    } else {
                        !is_scalar(v)
                    }
                })
                .map(|(k, _)| hit(join_key(&path, k)))
                .collect::<Vec<_>>()
        })
        .collect()
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
            Some(Ok(ms)) => !inside_range(h, &ms),
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
