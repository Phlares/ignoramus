//! The re-run recipe and the citation (design 2.1, items 13 and 14).
use super::sections::{EVENTS, Field, Outside, READINGS};
use crate::canon::sha256_hex;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Top-level files the manifest leaves out. `readings.md` changes when an author fills a slot;
/// the card must not. The box rewrites the two logs on every report read; the card names
/// the record log by count and last hash instead.
pub const SKIPPED: [&str; 3] = [READINGS, "records.jsonl", EVENTS];
/// The study folder's citation file.
pub const CITATION_FILE: &str = "CITATION.cff";

/// Every file of the study folder but the `skipped` ones, by relative path in sorted order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub skipped: Vec<String>,
    pub files: Vec<Outside>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub command: String,
    /// No record holds the box binary's hash (issue #67).
    pub box_binary_sha256: Field<String>,
    pub box_version: Field<String>,
    /// Missing until bundles are published.
    pub bundle_doi: Field<String>,
    pub manifest: Manifest,
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<Outside>) -> Result<(), String> {
    let fault = |e: std::io::Error| format!("{}: {e}", dir.display());
    let mut names = vec![];
    for entry in std::fs::read_dir(dir).map_err(fault)? {
        names.push(
            entry
                .map_err(fault)?
                .file_name()
                .to_string_lossy()
                .into_owned(),
        );
    }
    names.sort();
    for name in names {
        if prefix.is_empty() && SKIPPED.contains(&name.as_str()) {
            continue;
        }
        let path = format!("{prefix}{name}");
        let full = dir.join(&name);
        if full.is_dir() {
            walk(&full, &format!("{path}/"), out)?;
        } else {
            let bytes = std::fs::read(&full).map_err(|e| format!("{path}: {e}"))?;
            out.push(Outside {
                path,
                sha256: sha256_hex(&bytes),
            });
        }
    }
    Ok(())
}

/// The recipe of study `id`, whose folder is `dir` and whose analyses name `box_version`.
pub fn recipe(dir: &Path, id: &str, box_version: Option<String>) -> Result<Recipe, String> {
    let mut files = vec![];
    walk(dir, "", &mut files)?;
    Ok(Recipe {
        command: format!("ignorarium run {id}"),
        box_binary_sha256: Field::Missing { link: None },
        box_version: match box_version {
            Some(value) => Field::Recorded { value },
            None => Field::Missing { link: None },
        },
        bundle_doi: Field::Missing { link: None },
        manifest: Manifest {
            skipped: SKIPPED.map(String::from).to_vec(),
            files,
        },
    })
}

/// How to cite the study, from the keys of its `CITATION.cff`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Citation {
    pub bibtex: Field<String>,
    /// The keys read, as a `CITATION.cff` fragment.
    pub cff: Field<String>,
}

struct Author {
    family: String,
    given: String,
}

struct Cff {
    title: String,
    version: Option<String>,
    authors: Vec<Author>,
}

/// A plain or quoted scalar on one line. Refuses block, flow and anchor forms.
fn scalar(raw: &str) -> Result<String, String> {
    let v = raw.trim();
    let quoted = |q: char| v.len() >= 2 && v.starts_with(q) && v.ends_with(q);
    if quoted('"') || quoted('\'') {
        return Ok(v[1..v.len() - 1].to_string());
    }
    let bad = v.is_empty()
        || v.starts_with(['|', '>', '[', '{', '&', '*', '!', '"', '\''])
        || v.contains(" #");
    if bad {
        return Err(format!("cannot read the value `{v}`"));
    }
    Ok(v.to_string())
}

fn pair(line: &str) -> Result<(&str, &str), String> {
    line.split_once(':')
        .map(|(k, v)| (k.trim(), v))
        .ok_or_else(|| format!("cannot read the line `{line}`"))
}

fn once(slot: &mut Option<String>, key: &str, raw: &str) -> Result<(), String> {
    if slot.replace(scalar(raw)?).is_some() {
        return Err(format!("`{key}` appears twice"));
    }
    Ok(())
}

fn parse(text: &str) -> Result<Cff, String> {
    let (mut title, mut version) = (None, None);
    let mut authors: Vec<(Option<String>, Option<String>)> = vec![];
    let mut in_authors = false;
    let lines = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    for line in lines {
        if !line.starts_with([' ', '-']) {
            in_authors = false;
            let (key, raw) = pair(line)?;
            match key {
                "title" => once(&mut title, key, raw)?,
                "version" => once(&mut version, key, raw)?,
                "authors" if raw.trim().is_empty() && authors.is_empty() => in_authors = true,
                "authors" => return Err("cannot read `authors`".into()),
                _ => {}
            }
        } else if in_authors {
            let body = line.trim_start();
            let entry = match body.strip_prefix("- ") {
                Some(rest) => {
                    authors.push((None, None));
                    rest
                }
                None => body,
            };
            let slot = authors.last_mut().ok_or("cannot read `authors`")?;
            let (key, raw) = pair(entry)?;
            match key {
                "family-names" => once(&mut slot.0, key, raw)?,
                "given-names" => once(&mut slot.1, key, raw)?,
                "name" => {
                    return Err("an author with a `name` has no family and given names".into());
                }
                _ => {}
            }
        }
    }
    let authors = authors
        .into_iter()
        .map(|a| match a {
            (Some(family), Some(given)) => Ok(Author { family, given }),
            _ => Err("an author lacks `family-names` or `given-names`".to_string()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let title = title.ok_or("no `title`")?;
    if authors.is_empty() {
        return Err("no `authors`".into());
    }
    let values = [&title]
        .into_iter()
        .chain(&version)
        .chain(authors.iter().flat_map(|a| [&a.family, &a.given]));
    if values.into_iter().any(|v| v.contains(['{', '}', '\\'])) {
        return Err("a value holds a brace or backslash, which BibTeX would misread".into());
    }
    Ok(Cff {
        title,
        version,
        authors,
    })
}

impl Cff {
    fn bibtex(&self, key: &str) -> String {
        let names: Vec<String> = (self.authors.iter())
            .map(|a| format!("{}, {}", a.family, a.given))
            .collect();
        let mut fields = vec![
            format!("title = {{{}}}", self.title),
            format!("author = {{{}}}", names.join(" and ")),
        ];
        fields.extend(self.version.iter().map(|v| format!("version = {{{v}}}")));
        format!("@software{{{key},\n  {}\n}}\n", fields.join(",\n  "))
    }

    fn fragment(&self) -> String {
        let mut out = format!("title: \"{}\"\n", self.title);
        if let Some(v) = &self.version {
            out += &format!("version: {v}\n");
        }
        out += "authors:\n";
        for a in &self.authors {
            out += &format!(
                "  - family-names: {}\n    given-names: {}\n",
                a.family, a.given
            );
        }
        out
    }
}

/// The citation of study `id` from `CITATION.cff` in `dir`. A missing file gives `Missing`;
/// a file the line reader cannot read is refused.
pub fn citation(dir: &Path, id: &str) -> Result<Citation, String> {
    let text = match std::fs::read_to_string(dir.join(CITATION_FILE)) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Citation {
                bibtex: Field::Missing { link: None },
                cff: Field::Missing { link: None },
            });
        }
        Err(e) => return Err(format!("{CITATION_FILE}: {e}")),
    };
    let cff = parse(&text).map_err(|e| format!("{CITATION_FILE}: {e}"))?;
    Ok(Citation {
        bibtex: Field::Recorded {
            value: cff.bibtex(id),
        },
        cff: Field::Recorded {
            value: cff.fragment(),
        },
    })
}
