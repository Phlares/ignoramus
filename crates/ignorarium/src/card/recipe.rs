//! The re-run recipe and the citation (design 2.1, items 13 and 14).
use super::sections::Field;
use crate::canon::{sha256_file, sha256_hex};
use crate::files::optional;
use serde::{Deserialize, Serialize};
use std::fs::FileType;
use std::io;
use std::path::Path;

/// The study-folder names a re-run reads. A name ending in `/` is a tree. Nothing else in the
/// folder is an input: logs, `run/`, `deviations.jsonl` and `readings.md` are not listed.
pub const ROOTS: [&str; 7] = [
    "spec.toml",
    "spec.json",
    "spec.hash",
    "salt",
    "schedule.json",
    "in/",
    "payload/",
];
/// The study folder's citation file.
pub const CITATION_FILE: &str = "CITATION.cff";
/// What a re-run needs beside the command.
pub const REQUIRES: [&str; 3] = [
    "a box home where the study is registered",
    "the study folder as the working directory",
    "the box version recorded in `box_version`",
];

/// The re-run inputs of a study folder. `tree_sha256` is the SHA-256 of the `manifest_lines`
/// text: one line `<sha256>  <relative path>\n` per file, sorted, with `/` in paths. A symlink
/// is never followed; its line reads `link:<sha256 of the target text>  <path>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub roots: Vec<String>,
    /// Roots the folder does not hold.
    pub missing: Vec<String>,
    pub files: u64,
    pub tree_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub command: String,
    /// The isolation levels the rows hold, sorted. More than one is drift; the command uses the
    /// highest.
    pub isolation_levels: Vec<u8>,
    pub requires: Vec<String>,
    /// No record holds the box binary's hash (issue #67).
    pub box_binary_sha256: Field<String>,
    pub box_version: Field<String>,
    /// Missing until bundles are published.
    pub bundle_doi: Field<String>,
    pub manifest: Manifest,
}

pub(super) fn utf8(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("{}: the path is not UTF-8", path.display()))
}

/// Refuses a control character, which could forge a line of the manifest text.
pub(super) fn plain<'a>(text: &'a str, of: &str) -> Result<&'a str, String> {
    match text.chars().any(|c| c < ' ' || c == '\x7f') {
        true => Err(format!(
            "{}: {of} holds a control character",
            text.escape_debug()
        )),
        false => Ok(text),
    }
}

/// The entries of folder `full` (path `rel` in its tree) by UTF-8 name and type, unsorted.
pub(super) fn children(full: &Path, rel: &str) -> Result<Vec<(String, FileType)>, String> {
    let fault = |e: io::Error| format!("{rel}: {e}");
    let mut names = vec![];
    for e in std::fs::read_dir(full).map_err(fault)? {
        let e = e.map_err(fault)?;
        let name = e.file_name();
        let name = utf8(Path::new(&name))?.to_string();
        names.push((name, e.file_type().map_err(fault)?));
    }
    Ok(names)
}

fn entry(
    full: &Path,
    rel: &str,
    kind: FileType,
    out: &mut Vec<(String, String)>,
) -> Result<(), String> {
    plain(rel, "the path")?;
    let fault = |e: io::Error| format!("{rel}: {e}");
    if kind.is_symlink() {
        let target = std::fs::read_link(full).map_err(fault)?;
        let hash = sha256_hex(plain(utf8(&target)?, "the link target")?.as_bytes());
        out.push((format!("link:{hash}  {rel}"), rel.into()));
    } else if kind.is_dir() {
        for (name, kind) in children(full, rel)? {
            entry(&full.join(&name), &format!("{rel}/{name}"), kind, out)?;
        }
    } else if kind.is_file() {
        out.push((
            format!("{}  {rel}", sha256_file(full).map_err(fault)?),
            rel.into(),
        ));
    } else {
        return Err(format!("{rel}: not a file, directory or link"));
    }
    Ok(())
}

/// The lines of the study folder's manifest, and the roots it lacks.
fn walk(study_dir: &Path) -> Result<(Vec<String>, Vec<String>), String> {
    let (mut lines, mut missing) = (vec![], vec![]);
    for root in ROOTS {
        let name = root.trim_end_matches('/');
        match std::fs::symlink_metadata(study_dir.join(name)) {
            Ok(meta) => entry(&study_dir.join(name), name, meta.file_type(), &mut lines)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => missing.push(root.to_string()),
            Err(e) => return Err(format!("{name}: {e}")),
        }
    }
    lines.sort_by(|a, b| a.1.cmp(&b.1));
    let lines = lines.into_iter().map(|(line, _)| line).collect();
    Ok((lines, missing))
}

/// The sorted manifest lines of `study_dir`, one `<sha256>  <path>` per file, without the LF.
pub fn manifest_lines(study_dir: &Path) -> Result<Vec<String>, String> {
    walk(study_dir).map(|(lines, _)| lines)
}

/// The recipe of study `id`, whose folder is `dir`, whose rows hold `levels` (sorted, as
/// `Stack::isolation` holds them) and whose analysis, the one the card reads, names
/// `box_version`.
pub fn recipe(dir: &Path, id: &str, levels: &[u8], box_version: &str) -> Result<Recipe, String> {
    let (lines, missing) = walk(dir)?;
    let mut command = format!("ignorarium run {id}");
    if let Some(top) = levels.last() {
        command += &format!(" --max-level {top}");
    }
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    Ok(Recipe {
        command,
        isolation_levels: levels.to_vec(),
        requires: REQUIRES.map(String::from).to_vec(),
        box_binary_sha256: Field::of(None, None),
        box_version: Field::of(Some(box_version.into()), None),
        bundle_doi: Field::of(None, None),
        manifest: Manifest {
            roots: ROOTS.map(String::from).to_vec(),
            missing,
            files: lines.len() as u64,
            tree_sha256: sha256_hex(text.as_bytes()),
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

/// The citation of study `id` from `CITATION.cff` in the study folder `dir`. No folder or no
/// file gives `Missing`; a file the line reader cannot read is refused.
pub fn citation(dir: Option<&Path>, id: &str) -> Result<Citation, String> {
    let fault = |e: String| format!("{CITATION_FILE}: {e}");
    let text = dir.map_or(Ok(None), |d| {
        optional(std::fs::read_to_string(d.join(CITATION_FILE)))
    });
    let text = text.map_err(|e| fault(e.to_string()))?;
    let cff = text.as_deref().map(parse).transpose().map_err(fault)?;
    Ok(Citation {
        bibtex: Field::of(cff.as_ref().map(|c| c.bibtex(id)), None),
        cff: Field::of(cff.as_ref().map(Cff::fragment), None),
    })
}
