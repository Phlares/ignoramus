//! Publication steps per claim profile, read from `rules/publish.toml`.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    Required,
    Optional,
    Choice,
    OptIn,
    Listed,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Step {
    pub id: String,
    pub level: Level,
    pub note: String,
    pub file: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Profile {
    pub name: String,
    pub bytes: String,
    pub steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rules {
    labels: BTreeMap<String, String>,
    bytes: BTreeMap<String, String>,
    step: Vec<Row>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    id: String,
    exploratory: Level,
    replay: Level,
    confirmatory: Level,
    note: Option<String>,
    file: Option<String>,
}

fn rules() -> Rules {
    toml::from_str(include_str!("../../../rules/publish.toml"))
        .expect("rules/publish.toml is valid")
}

/// The profile `exploratory`, `replay` or `confirmatory`.
pub fn named(name: &str) -> Option<Profile> {
    let rules = rules();
    let steps = rules
        .step
        .into_iter()
        .map(|r| {
            let level = match name {
                "exploratory" => r.exploratory,
                "replay" => r.replay,
                _ => r.confirmatory,
            };
            Step {
                id: r.id,
                level,
                note: r.note.unwrap_or_default(),
                file: r.file,
            }
        })
        .collect();
    Some(Profile {
        name: name.to_string(),
        bytes: rules.bytes.get(name)?.clone(),
        steps,
    })
}

/// The profile for a claim label: signal, confirmed, replicated or generalized.
pub fn for_label(label: &str) -> Option<Profile> {
    named(rules().labels.get(label)?)
}
