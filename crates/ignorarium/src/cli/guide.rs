//! `guide`: the steps of a study, from `guide/steps.toml`.
use super::{CliError, Done, Run, out};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    id: String,
    title: String,
    does: String,
    produces: String,
    command: String,
}

#[derive(Deserialize)]
struct File {
    step: Vec<Step>,
}

pub fn run(json: bool) -> Run {
    let file: File = toml::from_str(include_str!("../../../../guide/steps.toml"))
        .map_err(|e| CliError::usage(format!("guide steps are not valid: {e}")))?;
    if json {
        let text =
            serde_json::to_string_pretty(&file.step).map_err(|e| CliError::usage(e.to_string()))?;
        out(&format!("{text}\n"))?;
    } else {
        for (i, s) in file.step.iter().enumerate() {
            out(&format!(
                "{}. {}: {}\n   does: {}\n   produces: {}\n   command: {}\n\n",
                i + 1,
                s.id,
                s.title,
                s.does,
                s.produces,
                s.command
            ))?;
        }
    }
    Ok(Done {
        code: 0,
        spec_hash: None,
        leaf: None,
    })
}
