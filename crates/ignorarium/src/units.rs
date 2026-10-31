//! Clusters and their units. Each selection or confirmation input is one cluster, named by
//! its path. The study takes up to `design.units_per_cluster` of the units the input
//! declares, by the design's pick rule, from a seed that the spec hash fixes.
use crate::seed::seed;
use crate::spec::model::{Pick, Role, Spec};
use evidence::rng::Rng;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cluster {
    pub path: String,
    pub stratum: String,
    pub role: Role,
    /// Unit indices into the input, ascending.
    pub picked: Vec<u32>,
}

/// One cluster per selection or confirmation input, in spec order. Its units come from the
/// seed `units/<path>`. An input without `units` gives none; the rules refuse it.
pub fn clusters(spec: &Spec, spec_hash: &[u8; 32]) -> Vec<Cluster> {
    let d = &spec.design;
    spec.inputs
        .iter()
        .filter(|i| matches!(i.role, Role::Selection | Role::Confirmation))
        .map(|i| Cluster {
            path: i.path.clone(),
            stratum: i.stratum.clone(),
            role: i.role,
            picked: pick(
                d.units_per_cluster,
                i.units.unwrap_or(0),
                d.pick,
                seed(spec_hash, &format!("units/{}", i.path)),
            ),
        })
        .collect()
}

/// `min(n, available)` indices of `0..available`, ascending. `First` takes 0, 1, 2, …;
/// `Seeded` takes the first positions of a Fisher-Yates shuffle drawn from `seed`
/// (`Rng::sample`).
pub fn pick(n: u32, available: u32, how: Pick, seed: [u8; 32]) -> Vec<u32> {
    let mut picked: Vec<u32> = match how {
        Pick::First => (0..n.min(available)).collect(),
        Pick::Seeded => Rng::from_seed(seed)
            .sample(available.into(), n.into())
            .into_iter()
            // Each value is below `available`, a u32.
            .map(|i| i as u32)
            .collect(),
    };
    picked.sort_unstable();
    picked
}

/// The name of a unit in rows and records: `<path>#<index>`.
pub fn key(path: &str, index: u32) -> String {
    format!("{path}#{index}")
}
