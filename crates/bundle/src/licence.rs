//! Per-file licence map, licence checks for a study's inputs, and the LICENSE.md text.
use crate::manifest::Entry;
use crate::{Problem, blank, tree::shown};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;

const HEADER_BYTES: u64 = 64 * 1024;
const HEADER_LINES: usize = 10;
const SPDX_TAG: &str = "SPDX-License-Identifier:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Common,
    Custom,
    RequestForm,
    Unspecified,
}

#[derive(Debug, Clone)]
pub struct Record {
    pub spdx: String,
    pub kind: Kind,
    pub source: String,
    pub creators: Vec<String>,
}

pub struct Input {
    pub path: String,
    pub sha256: String,
    pub licence: Record,
}

pub struct Licensing {
    pub code: String,
    pub data: String,
    pub inputs: Vec<Input>,
}

/// The input path as a bundle path, or `None` when it cannot name a bundle file.
fn normalize(path: &str) -> Option<&str> {
    let mut p = path;
    while let Some(rest) = p.strip_prefix("./") {
        p = rest;
    }
    let ok = !p.is_empty() && !p.contains('\\') && p.split('/').all(|s| !s.is_empty() && s != "..");
    ok.then_some(p)
}

/// `Some(text)` when the first 64 KiB of the file is UTF-8, a cut last character aside.
fn head_text(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(HEADER_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    match std::str::from_utf8(&bytes) {
        Ok(s) => Some(s.to_string()),
        Err(e) if e.error_len().is_none() => {
            Some(String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned())
        }
        Err(_) => None,
    }
}

fn cell(s: &str) -> String {
    s.replace("\r\n", " ")
        .replace(['\r', '\n'], " ")
        .replace('|', "\\|")
}

impl Licensing {
    fn input_for(&self, path: &str) -> Option<&Input> {
        self.inputs
            .iter()
            .find(|i| normalize(&i.path) == Some(path))
    }

    /// The SPDX id that covers `path`.
    pub fn of(&self, path: &str) -> &str {
        if let Some(i) = self.input_for(path) {
            &i.licence.spdx
        } else if path.starts_with("src/") {
            &self.code
        } else {
            &self.data
        }
    }

    /// Returns problems and warnings, each in input order, then licence texts, then headers.
    pub fn check(&self, study: &Path, files: &[Entry]) -> (Vec<Problem>, Vec<Problem>) {
        let mut problems = Vec::new();
        let mut warnings = Vec::new();
        for (i, input) in self.inputs.iter().enumerate() {
            let r = &input.licence;
            let mut gaps = Vec::new();
            if blank(&r.spdx) {
                gaps.push("the SPDX id is blank".to_string());
            }
            if blank(&r.source) {
                gaps.push("the source is blank".to_string());
            }
            if r.creators.is_empty() || r.creators.iter().any(|c| blank(c)) {
                gaps.push("a creator is missing or blank".to_string());
            }
            if r.kind == Kind::Custom && !r.spdx.starts_with("LicenseRef-") {
                gaps.push(format!(
                    "the custom licence id `{}` does not start with LicenseRef-",
                    shown(&r.spdx)
                ));
            }
            if !gaps.is_empty() {
                problems.push(Problem::new(
                    "E-PUB-005",
                    format!("inputs[{i}].licence"),
                    format!("The licence record is incomplete: {}", gaps.join("; ")),
                    "Fill the input's licence record: an SPDX id, the source URL and every creator; a custom licence uses a LicenseRef- id.",
                ));
            }
            let entry = normalize(&input.path).and_then(|p| files.iter().find(|f| f.path == p));
            let no_share = matches!(r.kind, Kind::RequestForm | Kind::Unspecified);
            match entry {
                Some(e) => {
                    let path = shown(&e.path);
                    if no_share {
                        problems.push(Problem::new(
                            "E-PUB-006",
                            path.clone(),
                            "The file is an input whose licence does not allow sharing it",
                            "Leave the file out of the bundle and cite its source, or record a licence that allows sharing it.",
                        ));
                    }
                    if !e.sha256.eq_ignore_ascii_case(&input.sha256) {
                        problems.push(Problem::new(
                            "E-PUB-011",
                            path,
                            "The file differs from the registered input",
                            "Restore the registered file, or register an amendment with the new hash.",
                        ));
                    }
                }
                None if no_share => warnings.push(Problem::new(
                    "W-PUB-001",
                    format!("inputs[{i}]"),
                    "The input is cited only and its licence does not allow sharing it",
                    "Check that the published results may use this input.",
                )),
                None => {}
            }
        }
        let ids: BTreeSet<&str> = files.iter().map(|f| self.of(&f.path)).collect();
        for id in ids {
            let want = format!("LICENSES/{id}.txt");
            if !files.iter().any(|f| f.path == want) {
                let shown_want = shown(&want);
                problems.push(Problem::new(
                    "E-PUB-007",
                    shown_want.clone(),
                    "The licence text is missing",
                    format!(
                        "Add the licence text as {shown_want} (https://reuse.software/spec-3.3/)."
                    ),
                ));
            }
        }
        for f in files.iter().filter(|f| f.path.starts_with("src/")) {
            let Some(text) = head_text(&study.join(&f.path)) else {
                continue;
            };
            if !text
                .lines()
                .take(HEADER_LINES)
                .any(|l| l.contains(SPDX_TAG))
            {
                warnings.push(Problem::new(
                    "W-PUB-002",
                    shown(&f.path),
                    format!("The file has no {SPDX_TAG} line in its first {HEADER_LINES} lines"),
                    "Add an SPDX-License-Identifier line near the top of the file.",
                ));
            }
        }
        (problems, warnings)
    }

    /// The LICENSE.md text.
    pub fn statement(&self, files: &[Entry]) -> String {
        let (code, data) = (&self.code, &self.data);
        let mut out = format!(
            "# Licences\n\n\
Files under `src/` are licensed under {code} (`LICENSES/{code}.txt`).\n\
All other files are licensed under {data} (`LICENSES/{data}.txt`), except the inputs below.\n\
{data} covers only the rights of this study's authors.\n\n\
## Inputs\n\n\
These files came from others and keep their own licences.\n\n"
        );
        let mut rows: Vec<(&str, &Input)> = self
            .inputs
            .iter()
            .filter_map(|i| {
                let p = normalize(&i.path)?;
                files.iter().any(|f| f.path == p).then_some((p, i))
            })
            .collect();
        rows.sort_by_key(|(p, _)| *p);
        if rows.is_empty() {
            out.push_str("None of the files came from others.\n");
            return out;
        }
        out.push_str("| File | Licence | Source | Creators |\n|---|---|---|---|\n");
        for (p, i) in rows {
            let r = &i.licence;
            out.push_str(&format!(
                "| `{}` | {} | {} | {} |\n",
                cell(p),
                cell(&r.spdx),
                cell(&r.source),
                cell(&r.creators.join("; "))
            ));
        }
        out
    }
}
