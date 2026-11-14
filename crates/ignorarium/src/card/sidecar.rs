//! The files `report --card` writes beside the card: the signed input list and the observations
//! the tests read. The card keeps their counts and hashes.
use super::HypothesisCard;
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
    /// Inputs the `inputs` record names without a digest: absent when the study began.
    pub absent: u64,
    pub list_sha256: String,
    pub file: String,
}

/// A registered verdict whose e-value was forced to 0, so the file's observations do not give it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Forced {
    pub hypothesis: String,
    pub reason: String,
}

/// The records of the observations file (the header apart), its SHA-256, and the verdicts
/// whose e-value the file does not reproduce.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observations {
    pub rows: u64,
    pub sha256: String,
    pub file: String,
    pub forced: Vec<Forced>,
}

/// The text of each sidecar file, and the signed input lines it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Sidecars {
    pub lines: Vec<String>,
    pub absent: u64,
    pub rows: u64,
    pub data: String,
    pub observations: String,
}

/// The `<sha256>  <path>` line of each present input of the `inputs` record, sorted by path
/// as the manifest is, each path spelled as the input view holds it.
fn signed_lines(log: &Log) -> Result<(Vec<String>, u64), String> {
    let frozen = (log.records.iter().find_map(Inputs::of)).map_or_else(Vec::new, |i| i.inputs);
    let absent = frozen.iter().filter(|(_, hash)| hash.is_none()).count() as u64;
    let mut pairs: Vec<(String, String)> = (frozen.into_iter())
        .filter_map(|(path, hash)| Some((view::key(&path), hash?)))
        .collect();
    pairs.sort();
    for (path, _) in &pairs {
        plain(path, "an input path")?;
    }
    let lines = (pairs.into_iter())
        .map(|(path, hash)| format!("{hash}  {path}"))
        .collect();
    Ok((lines, absent))
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
        let (lines, absent) = signed_lines(log)?;
        let observed = source.observations()?;
        Ok(Sidecars {
            data: list_text(&lines),
            observations: csv(&observed),
            rows: observed.len() as u64,
            absent,
            lines,
        })
    }

    pub fn data_list(&self) -> DataList {
        DataList {
            count: self.lines.len() as u64,
            absent: self.absent,
            list_sha256: list_sha256(&self.lines),
            file: DATA_FILE.into(),
        }
    }

    /// The observations entry of the card whose verdicts are `verdicts`.
    pub fn observations_card(&self, verdicts: &[HypothesisCard]) -> Observations {
        let forced = (verdicts.iter())
            .filter(|v| v.basis == "test" && v.e_value == Some(0.0))
            .filter_map(|v| {
                let reason = v.reason.clone()?;
                Some(Forced {
                    hypothesis: v.hypothesis.clone(),
                    reason,
                })
            })
            .collect();
        Observations {
            rows: self.rows,
            sha256: crate::canon::sha256_hex(self.observations.as_bytes()),
            file: OBSERVATIONS_FILE.into(),
            forced,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The records of RFC 4180 `text`.
    fn parse(text: &str) -> Vec<Vec<String>> {
        let (mut rows, mut row, mut cell) = (vec![], vec![], String::new());
        let (mut quoted, mut chars) = (false, text.chars().peekable());
        while let Some(c) = chars.next() {
            match (quoted, c) {
                (true, '"') if chars.peek() == Some(&'"') => {
                    cell.push('"');
                    chars.next();
                }
                (true, '"') => quoted = false,
                (false, '"') => quoted = true,
                (false, ',') => row.push(std::mem::take(&mut cell)),
                (false, '\n') => {
                    row.push(std::mem::take(&mut cell));
                    rows.push(std::mem::take(&mut row));
                }
                (_, c) => cell.push(c),
            }
        }
        rows
    }

    #[test]
    fn a_cluster_path_with_a_comma_and_a_quote_round_trips() {
        let path = "src/a,\"b\".txt\nx";
        let row = Observed {
            hypothesis: "H1".into(),
            stratum: "s,1".into(),
            side: "below",
            position: 1,
            cluster: path.into(),
            units: 5,
            observation: 0.25,
        };
        let rows = parse(&csv(&[row]));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1], ["H1", "s,1", "below", "1", path, "5", "0.25"]);
    }
}
