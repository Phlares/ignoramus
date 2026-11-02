//! The blinded re-size of a study that declares one: once its first batch has run, how many
//! reserve clusters each group a claim is tested in appends, from the first batch's row
//! records and the spec hash alone.
use crate::analysis::{Paired, claim_strata, group, paired};
use crate::derived::{CONTINUE, GroupSd, RESIZE, Register, Resize, ResizeStratum, STOP};
use crate::records::Record;
use crate::rules::REPS;
use crate::runner::RunError;
use crate::schedule::{RESERVE_STAGE, Row, reserve_order};
use crate::seed::seed;
use crate::spec::model::{Role, Spec};
use crate::units::Cluster;
use evidence::claim::{Bound, Statistic, observation};
use evidence::power::{Plan, blinded_sd, resize_count};
use std::collections::{BTreeMap, BTreeSet};

/// Why a study whose reserve falls short stops.
pub const SHORT: &str = "the reserve cannot reach the power the claim needs";

/// The re-size of `spec` from the first batch in `records`, over the study's `clusters`
/// (`analysis::distinct_clusters`). For each hypothesis with a claim, and each group it is
/// tested in (`""` pools every cluster): the observation of the claim's first bound
/// (`claim::observation`) for each first-batch cluster, over its complete units; their
/// blinded sd `s` (`blinded_sd`, about 0); and `resize_count` at `power_at` for
/// `design.power`^(1/k) (k groups), at the study's level x `alpha_share`, `REPS` replicates,
/// seed `resize/<id>/<group>`, over the group's first-batch then reserve clusters. A group
/// needs the largest count its claims give. It appends what it needs beyond its first batch.
/// A group whose reserve cannot reach its count stops the study, which then appends nothing.
/// `Err` when a first-batch unit is incomplete, or the study has no level or the count cannot
/// be made.
pub fn decide(
    spec: &Spec,
    spec_hash: &[u8; 32],
    records: &[Record],
    clusters: &[Cluster],
) -> Result<Resize, String> {
    let level = (records.iter().find_map(Register::of))
        .and_then(|r| r.alpha)
        .map(|a| a.level)
        .ok_or("the study holds no level")?;
    let pairs = paired(spec, records);
    let repeats = spec.design.repeats;
    let mut groups = groups(spec, clusters);
    for h in &spec.question.hypotheses {
        let Some(claim) = h.claim else { continue };
        let lacks = |what: &str| format!("hypothesis {} has no {what}", h.id);
        let stat = (h.statistic.as_ref())
            .ok_or_else(|| lacks("statistic"))?
            .to_evidence();
        let at = h.power_at.ok_or_else(|| lacks("power_at"))?;
        let share = h.alpha_share.ok_or_else(|| lacks("alpha_share"))?;
        let bounds = claim.bounds();
        let first = bounds[0];
        let strata = claim_strata(h);
        let target = spec.design.power.powf(1.0 / strata.len() as f64);
        let plan = Plan {
            stat,
            bounds,
            strata: vec![],
            level: level * share,
        };
        for g in strata {
            let (samples, incomplete) = group(
                clusters,
                Role::Confirmation,
                g,
                &h.outcome,
                &h.contrast,
                &pairs,
                repeats,
            );
            if incomplete > 0 {
                return Err(format!("incomplete units in the first batch: {incomplete}"));
            }
            let s = spread(&stat, &first, &samples)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("group {g:?} has no observation"))?;
            let sizes: Vec<u32> = (clusters.iter())
                .filter(|c| holds(g, c, Role::Confirmation))
                .chain(clusters.iter().filter(|c| holds(g, c, Role::Reserve)))
                .map(|c| u32::try_from(c.picked.len()).unwrap_or(u32::MAX))
                .collect();
            let cap = u32::try_from(sizes.len()).unwrap_or(u32::MAX);
            let draw = seed(spec_hash, &format!("resize/{}/{g}", h.id));
            let n = resize_count(&plan, &sizes, s, at, target, REPS, draw, cap)
                .map_err(|e| e.to_string())?;
            if let Some(counted) = groups.get_mut(g) {
                counted.needed = counted.needed.zip(n).map(|(a, b)| a.max(u64::from(b)));
            }
        }
    }
    let go = groups.values().all(|g| g.needed.is_some());
    let strata = (groups.into_values())
        .map(|g| ResizeStratum {
            appended: (g.needed.filter(|_| go)).map_or(0, |n| n.saturating_sub(g.first)),
            ..g
        })
        .collect();
    Ok(Resize {
        strata,
        decision: if go { CONTINUE } else { STOP }.into(),
        reason: (!go).then(|| SHORT.into()),
    })
}

/// `decide`, or for its `Err`, a stop with that reason that counts each group's clusters and
/// needs none.
pub fn or_stop(
    spec: &Spec,
    spec_hash: &[u8; 32],
    records: &[Record],
    clusters: &[Cluster],
) -> Resize {
    decide(spec, spec_hash, records, clusters).unwrap_or_else(|why| Resize {
        strata: (groups(spec, clusters).into_values())
            .map(|g| ResizeStratum { needed: None, ..g })
            .collect(),
        decision: STOP.into(),
        reason: Some(why),
    })
}

/// The study's `resize` record; `None` when its records hold none. `Err` when it does not
/// parse.
pub fn recorded(records: &[Record]) -> Result<Option<Resize>, RunError> {
    (records.iter().find(|r| r.kind == RESIZE))
        .map(|r| {
            Resize::of(r)
                .ok_or_else(|| RunError::Records("the resize record does not parse".into()))
        })
        .transpose()
}

/// The paths of the reserve clusters of `clusters` whose rows run: those `resize`, the study's
/// record, admits; every one while it has none, so a study waits for its record.
pub fn ran<'a>(
    spec: &'a Spec,
    clusters: &'a [Cluster],
    resize: Option<&Resize>,
) -> BTreeSet<&'a str> {
    let order = reserve_order(spec, clusters);
    match resize {
        Some(r) => r.admitted(&order),
        None => order.iter().map(|c| c.path.as_str()).collect(),
    }
}

/// `row` runs: it is no reserve row, or its cluster is one that `ran` holds.
pub fn runs(row: &Row, ran: &BTreeSet<&str>) -> bool {
    row.stage != RESERVE_STAGE || ran.contains(row.cluster.as_str())
}

/// What a study that re-sized shows of its spread once it is over: for each hypothesis with
/// a claim and each group it is tested in, the blinded sd of the observations of the claim's
/// first bound, as the re-size reads them, over the first batch's complete units, and over
/// the appended reserve clusters' (`clusters` holds those that ran; `None` for none).
pub fn spreads(spec: &Spec, pairs: &Paired, clusters: &[Cluster]) -> (Vec<GroupSd>, Vec<GroupSd>) {
    let repeats = spec.design.repeats;
    let mut out = (vec![], vec![]);
    for h in &spec.question.hypotheses {
        let (Some(claim), Some(stat)) = (h.claim, h.statistic.as_ref()) else {
            continue;
        };
        let stat = stat.to_evidence();
        let first = claim.bounds()[0];
        for g in claim_strata(h) {
            let sd = |role| {
                let (samples, _) =
                    group(clusters, role, g, &h.outcome, &h.contrast, pairs, repeats);
                GroupSd {
                    hypothesis: h.id.clone(),
                    stratum: g.into(),
                    sd: spread(&stat, &first, &samples).ok().flatten(),
                }
            };
            out.0.push(sd(Role::Confirmation));
            out.1.push(sd(Role::Reserve));
        }
    }
    out
}

/// The blinded sd of the observations (`claim::observation`) of `bound` over `samples`, one
/// per cluster.
fn spread(
    stat: &Statistic,
    bound: &Bound,
    samples: &[Vec<f64>],
) -> Result<Option<f64>, evidence::Error> {
    let obs = (samples.iter())
        .map(|xs| observation(stat, bound, xs))
        .collect::<Result<Vec<f64>, _>>()?;
    Ok(blinded_sd(&obs))
}

/// Each group a claim is tested in, by name, with its first-batch and reserve clusters
/// counted, needing 0 and appending none.
fn groups<'a>(spec: &'a Spec, clusters: &[Cluster]) -> BTreeMap<&'a str, ResizeStratum> {
    let claimed = (spec.question.hypotheses.iter())
        .filter(|h| h.claim.is_some())
        .flat_map(claim_strata);
    let count = |g: &str, role| clusters.iter().filter(|c| holds(g, c, role)).count() as u64;
    claimed
        .map(|g| {
            let stratum = ResizeStratum {
                stratum: g.into(),
                first: count(g, Role::Confirmation),
                reserve: count(g, Role::Reserve),
                needed: Some(0),
                appended: 0,
            };
            (g, stratum)
        })
        .collect()
}

/// Group `g` holds cluster `c` of `role`: the pooled group `""` holds every one.
fn holds(g: &str, c: &Cluster, role: Role) -> bool {
    c.role == role && (g.is_empty() || c.stratum == g)
}
