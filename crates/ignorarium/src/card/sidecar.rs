//! The files `report --card` writes beside the card: the signed input list and the observations
//! the tests read. The card keeps their counts and hashes.
use super::recipe::{list_sha256, list_text, plain};
use crate::analysis::{Observed, Source};
use crate::derived::Inputs;
use crate::home::Home;
use crate::records::Log;
use crate::view;
use serde::{Deserialize, Serialize};

/// The signed input list as a file.
pub const DATA_FILE: &str = "data.sha256";
/// The observations as a file.
pub const OBSERVATIONS_FILE: &str = "observations.csv";
const HEADER: &str = "hypothesis,stratum,side,position,cluster,units,observation\n";

/// The signed input list: its line count and the SHA-256 of `file`, its sorted lines
/// `<sha256>  <path>` (`recipe::list_sha256`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataList {
    pub count: u64,
    pub list_sha256: String,
    pub file: String,
}

/// The rows of the observations file and its SHA-256.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observations {
    pub rows: u64,
    pub sha256: String,
    pub file: String,
}

/// The text of each sidecar file, and the signed input lines it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Sidecars {
    pub lines: Vec<String>,
    pub data: String,
    pub observations: String,
}

/// The `<sha256>  <path>` line of each present input of the `inputs` record, sorted by path
/// as the manifest is, each path spelled as the input view holds it.
fn signed_lines(log: &Log) -> Result<Vec<String>, String> {
    let frozen = (log.records.iter().find_map(Inputs::of)).map_or_else(Vec::new, |i| i.inputs);
    let mut pairs: Vec<(String, String)> = (frozen.into_iter())
        .filter_map(|(path, hash)| Some((view::key(&path), hash?)))
        .collect();
    pairs.sort();
    for (path, _) in &pairs {
        plain(path, "an input path")?;
    }
    Ok(pairs
        .into_iter()
        .map(|(path, hash)| format!("{hash}  {path}"))
        .collect())
}

fn cell(text: &str) -> String {
    match text.contains([',', '"', '\n', '\r']) {
        true => format!("\"{}\"", text.replace('"', "\"\"")),
        false => text.into(),
    }
}

fn csv(rows: &[Observed]) -> String {
    let row = |o: &Observed| {
        format!(
            "{},{},{},{},{},{},{}\n",
            cell(&o.hypothesis),
            cell(&o.stratum),
            o.side,
            o.position,
            cell(&o.cluster),
            o.units,
            o.observation
        )
    };
    rows.iter().fold(HEADER.to_string(), |s, o| s + &row(o))
}

impl Sidecars {
    /// The sidecars of the study `id` whose records are `log`, read against the plan its folder
    /// stores.
    pub fn of(home: &Home, id: &str, log: &Log) -> Result<Sidecars, String> {
        let text = std::fs::read(home.study_dir(id).join("spec.toml"))
            .map_err(|e| format!("its plan: {e}"))?;
        let source = Source::read(&text, &log.records, log.data_hashes.as_ref())?;
        Sidecars::from_source(&source, log)
    }

    /// The sidecars of `log`, whose observations `source` reads.
    pub fn from_source(source: &Source, log: &Log) -> Result<Sidecars, String> {
        let lines = signed_lines(log)?;
        Ok(Sidecars {
            data: list_text(&lines),
            observations: csv(&source.observations()?),
            lines,
        })
    }

    pub fn data_list(&self) -> DataList {
        DataList {
            count: self.lines.len() as u64,
            list_sha256: list_sha256(&self.lines),
            file: DATA_FILE.into(),
        }
    }

    pub fn observations_card(&self) -> Observations {
        Observations {
            rows: self.observations.lines().count() as u64 - 1,
            sha256: crate::canon::sha256_hex(self.observations.as_bytes()),
            file: OBSERVATIONS_FILE.into(),
        }
    }
}
