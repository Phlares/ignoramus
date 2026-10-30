//! Builds a study's bundle folder from the study folder, the run facts and the release facts.
use crate::citation::{self, Release, Tool};
use crate::licence::{HEADER_BYTES, Licensing};
use crate::manifest::{self, Entry, sha256_bytes, sha256_copy};
use crate::profile::{self, Level, Profile};
use crate::rocrate::{self, Artifact, Crate, Run};
use crate::tree::{self, Found, name_is_safe, shown};
use crate::{Error, Problem, yaml};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Files the build writes. A file of such a name in the study is not copied.
pub const GENERATED: [&str; 7] = [
    "ARTIFACTS.sha256",
    "CITATION.cff",
    "LICENSE.md",
    manifest::NAME,
    "datacite.json",
    "publish-profile.json",
    rocrate::DESCRIPTOR,
];

const REQUIRED_FILES: [&str; 3] = ["README.md", "citation.toml", "spec.toml"];
const REQUIRED_FOLDERS: [&str; 2] = ["LICENSES/", "results/"];
const FOLDERS: [&str; 6] = ["LICENSES", "data", "doc", "records", "results", "src"];
const ATTACH_FOLDERS: [&str; 3] = ["doc/", "records/", "results/"];

/// What the caller knows about the study: its runs, licences and extra files.
pub struct Facts {
    pub study_id: String,
    pub label: String,
    pub licensing: Licensing,
    pub tool: Tool,
    pub runs: Vec<Run>,
    /// Extra bundle files as (path, bytes).
    pub attachments: Vec<(String, Vec<u8>)>,
    /// Files that go beside the bundle, not in it.
    pub artifacts: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct Built {
    pub dir: PathBuf,
    pub manifest_sha256: String,
    pub artifacts: Vec<(Artifact, PathBuf)>,
    pub warnings: Vec<Problem>,
    pub profile: Profile,
}

pub fn zip_name(study_id: &str, version: &str) -> String {
    format!("{study_id}-{version}.zip")
}

/// Each `before` file must appear in `after` with the same hash.
fn verify_copied(before: &[Entry], after: &[Entry]) -> io::Result<()> {
    let now: BTreeMap<&str, &str> = after
        .iter()
        .map(|e| (e.path.as_str(), e.sha256.as_str()))
        .collect();
    for e in before {
        if now.get(e.path.as_str()) != Some(&e.sha256.as_str()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} changed while the bundle was built", shown(&e.path)),
            ));
        }
    }
    Ok(())
}

fn top(path: &str) -> &str {
    path.split_once('/').map_or(path, |(first, _)| first)
}

fn unknown_top_level(files: &[Found]) -> Vec<Problem> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for f in files {
        let name = top(&f.path);
        let known = if f.path.contains('/') {
            FOLDERS.contains(&name)
        } else {
            REQUIRED_FILES.contains(&name)
        };
        if !known && seen.insert(name) {
            out.push(Problem::new(
                "E-PUB-003",
                shown(name),
                "is not an allowed top-level file or folder",
                "Move it into data/, doc/, records/, results/ or src/.",
            ));
        }
    }
    out
}

/// True when `p` is a safe relative path in doc/, records/ or results/.
fn attachment_path_ok(p: &str) -> bool {
    ATTACH_FOLDERS.iter().any(|d| p.starts_with(d))
        && name_is_safe(p)
        && p.split('/').all(|s| !matches!(s, "" | "." | ".."))
}

fn is_artifact_name(n: &str) -> bool {
    !n.is_empty()
        && n.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn check_attachments(attachments: &[(String, Vec<u8>)], study: &[Entry]) -> Vec<Problem> {
    let mut taken: Vec<&str> = study.iter().map(|e| e.path.as_str()).collect();
    taken.extend(GENERATED);
    let mut out = Vec::new();
    for (i, (path, _)) in attachments.iter().enumerate() {
        let nested = |a: &str, b: &str| b.strip_prefix(a).is_some_and(|r| r.starts_with('/'));
        let clash = taken
            .iter()
            .any(|t| *t == path || nested(t, path) || nested(path, t))
            || attachments[..i].iter().any(|(o, _)| o == path)
            || attachments[i + 1..]
                .iter()
                .any(|(o, _)| nested(o, path) || nested(path, o));
        if !attachment_path_ok(path) || clash {
            out.push(Problem::new(
                "E-PUB-010",
                shown(path),
                "cannot be attached",
                "Attach under a new relative path in doc/, records/ or results/.",
            ));
        }
    }
    out
}

fn hash_artifacts(
    paths: &[PathBuf],
    zip: &str,
    problems: &mut Vec<Problem>,
) -> Result<Vec<(Artifact, PathBuf)>, Error> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for path in paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !is_artifact_name(&name) || name == zip || !seen.insert(name.clone()) {
            problems.push(Problem::new(
                "E-PUB-010",
                shown(&name),
                "cannot be an artifact name",
                "Name artifacts with letters, digits, dots, hyphens and underscores, each name once.",
            ));
            continue;
        }
        let opened = File::open(path)
            .ok()
            .filter(|f| f.metadata().is_ok_and(|m| m.is_file()));
        let Some(mut file) = opened else {
            problems.push(Problem::new(
                "E-PUB-010",
                shown(&name),
                "cannot be read",
                "Give the path of a regular file that can be read.",
            ));
            continue;
        };
        let (sha256, size) = sha256_copy(&mut file, &mut io::sink())?;
        out.push((Artifact { name, sha256, size }, path.clone()));
    }
    out.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    Ok(out)
}

fn missing_files(names: &[&str]) -> Vec<Problem> {
    let mut out = Vec::new();
    let mut want: Vec<&str> = REQUIRED_FILES.to_vec();
    want.extend(REQUIRED_FOLDERS);
    for w in want {
        let found = if w.ends_with('/') {
            names.iter().any(|n| n.starts_with(w))
        } else {
            names.contains(&w)
        };
        if !found {
            out.push(Problem::new(
                "E-PUB-002",
                w,
                "is missing",
                "Add it: a study folder holds README.md, citation.toml, spec.toml, LICENSES/ and results/.",
            ));
        }
    }
    out
}

/// A sink that keeps the first `keep` bytes written to it.
struct Prefix {
    bytes: Vec<u8>,
    keep: usize,
}

impl Write for Prefix {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = buf.len().min(self.keep - self.bytes.len());
        self.bytes.extend_from_slice(&buf[..n]);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Writes `src` to the new file `out/path` and returns its manifest entry.
fn put(out: &Path, path: &str, src: &mut impl Read) -> io::Result<Entry> {
    let dest = out.join(path);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let (sha256, size) = sha256_copy(src, &mut File::create_new(dest)?)?;
    Ok(Entry {
        path: path.to_string(),
        sha256,
        size,
    })
}

/// `p` with its parent made canonical, through the parent's deepest existing ancestor.
fn anchored(p: &Path) -> io::Result<PathBuf> {
    let (Some(parent), Some(name)) = (p.parent(), p.file_name()) else {
        return p.canonicalize();
    };
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let base = parent.canonicalize().or_else(|_| anchored(parent))?;
    Ok(base.join(name))
}

fn json_text(v: &impl serde::Serialize) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("a JSON tree serialises");
    s.push('\n');
    s
}

/// Builds the bundle folder `out` (which must not exist) from the study folder.
/// Checks every rule first and returns every problem at once; `out` is not made then.
pub fn build(study: &Path, out: &Path, facts: &Facts, release: &Release) -> Result<Built, Error> {
    let (mut found, mut problems) = tree::walk(study)?;
    found.retain(|f| f.path.contains('/') || !GENERATED.contains(&f.path.as_str()));
    problems.extend(unknown_top_level(&found));

    let mut citation = None;
    if let Some(f) = found.iter().find(|f| f.path == "citation.toml") {
        let mut bytes = Vec::new();
        tree::open(study, f)?.read_to_end(&mut bytes)?;
        match citation::parse_bytes(&bytes) {
            Ok(c) => citation = Some(c),
            Err(p) => problems.extend(p),
        }
    }

    problems.extend(citation::check_release(release));
    let profile = profile::for_label(&facts.label);
    if profile.is_none() {
        problems.push(Problem::new(
            "E-PUB-014",
            "label",
            "is not a claim label",
            "Use one of the claim labels: signal, confirmed, replicated, generalized.",
        ));
    }
    if !is_artifact_name(&facts.study_id) {
        problems.push(Problem::new(
            "E-PUB-016",
            "study_id",
            "cannot name a bundle zip",
            "Use letters, digits, dots, hyphens and underscores in the study id.",
        ));
    }

    let mut files = Vec::with_capacity(found.len());
    let mut heads = BTreeMap::new();
    for f in &found {
        let mut head = Prefix {
            bytes: Vec::new(),
            keep: if f.path.starts_with("src/") {
                HEADER_BYTES
            } else {
                0
            },
        };
        let (sha256, size) = sha256_copy(&mut tree::open(study, f)?, &mut head)?;
        heads.insert(f.path.as_str(), head.bytes);
        files.push(Entry {
            path: f.path.clone(),
            sha256,
            size,
        });
    }
    problems.extend(check_attachments(&facts.attachments, &files));
    let artifacts = hash_artifacts(
        &facts.artifacts,
        &zip_name(&facts.study_id, &release.version),
        &mut problems,
    )?;

    let mut all = files.clone();
    all.extend(facts.attachments.iter().map(|(path, bytes)| Entry {
        path: path.clone(),
        sha256: sha256_bytes(bytes),
        size: bytes.len() as u64,
    }));
    all.sort_by(|a, b| a.path.cmp(&b.path));
    let mut names: Vec<&str> = all.iter().map(|e| e.path.as_str()).collect();
    problems.extend(missing_files(&names));
    let (licence_problems, warnings) = facts
        .licensing
        .check(&all, &|p| heads.get(p).map(Vec::as_slice));
    problems.extend(licence_problems);
    names.extend(GENERATED);
    problems.extend(rocrate::missing(&facts.runs, &names));
    for step in profile.iter().flat_map(|p| &p.steps) {
        if let (Level::Required, Some(file)) = (step.level, &step.file)
            && !names.contains(&file.as_str())
        {
            problems.push(Problem::new(
                "E-PUB-009",
                shown(file),
                "is a file a required step leaves",
                format!("Attach the file the profile names: {file}."),
            ));
        }
    }

    if anchored(out)?.starts_with(study.canonicalize()?) {
        problems.push(Problem::new(
            "E-PUB-010",
            "out",
            "lies inside the study folder",
            "Build the bundle in a folder outside the study folder.",
        ));
    }

    let (true, Some(citation), Some(profile)) = (problems.is_empty(), citation, profile) else {
        return Err(Error::Refused { problems, warnings });
    };

    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir(out)?;
    let mut listed = Vec::with_capacity(all.len() + GENERATED.len());
    for f in &found {
        listed.push(put(out, &f.path, &mut tree::open(study, f)?)?);
    }
    verify_copied(&files, &listed)?;
    for (path, bytes) in &facts.attachments {
        listed.push(put(out, path, &mut bytes.as_slice())?);
    }
    let data = &facts.licensing.data;
    let cff = citation::cff(&citation, release, &facts.tool, data);
    let datacite = citation::datacite(&citation, release, &facts.tool, data);
    let artifact_list: Vec<Entry> = artifacts
        .iter()
        .map(|(a, _)| Entry {
            path: a.name.clone(),
            sha256: a.sha256.clone(),
            size: a.size,
        })
        .collect();
    for (name, text) in [
        ("CITATION.cff", yaml::block(&cff)),
        ("datacite.json", json_text(&datacite)),
        ("LICENSE.md", facts.licensing.statement(&all)),
        ("publish-profile.json", json_text(&profile)),
        ("ARTIFACTS.sha256", manifest::render(&artifact_list)),
    ] {
        listed.push(put(out, name, &mut text.as_bytes())?);
    }
    listed.sort_by(|a, b| a.path.cmp(&b.path));

    let plain: Vec<Artifact> = artifacts.iter().map(|(a, _)| a.clone()).collect();
    let doc = rocrate::metadata(&Crate {
        study_id: &facts.study_id,
        citation: &citation,
        release,
        tool: &facts.tool,
        licensing: &facts.licensing,
        files: &listed,
        runs: &facts.runs,
        artifacts: &plain,
    });
    listed.push(put(
        out,
        rocrate::DESCRIPTOR,
        &mut json_text(&doc).as_bytes(),
    )?);
    listed.sort_by(|a, b| a.path.cmp(&b.path));
    let text = manifest::render(&listed);
    let manifest = put(out, manifest::NAME, &mut text.as_bytes())?;

    Ok(Built {
        dir: out.to_path_buf(),
        manifest_sha256: manifest.sha256,
        artifacts,
        warnings,
        profile,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, sha: &str) -> Entry {
        Entry {
            path: path.into(),
            sha256: sha.into(),
            size: 1,
        }
    }

    #[test]
    fn matching_copies_pass() {
        let before = [entry("a", "1"), entry("data/texts/a.txt", "2")];
        let after = [
            entry("MANIFEST.sha256", "9"),
            entry("a", "1"),
            entry("data/texts/a.txt", "2"),
        ];
        assert!(verify_copied(&before, &after).is_ok());
    }

    #[test]
    fn a_changed_copy_is_refused() {
        let before = [entry("a", "1"), entry("data/texts/a.txt", "2")];
        let after = [entry("a", "1"), entry("data/texts/a.txt", "3")];
        let e = verify_copied(&before, &after).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            e.to_string(),
            "data/texts/a.txt changed while the bundle was built"
        );
    }

    #[test]
    fn a_missing_copy_is_refused() {
        let e = verify_copied(&[entry("a", "1")], &[]).unwrap_err();
        assert_eq!(e.to_string(), "a changed while the bundle was built");
    }

    #[test]
    fn anchored_resolves_through_the_deepest_existing_folder() {
        let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
        assert_eq!(anchored(Path::new("x")).unwrap(), cwd.join("x"));
        assert_eq!(
            anchored(Path::new("no-such/x")).unwrap(),
            cwd.join("no-such/x")
        );
        assert_eq!(anchored(Path::new("src/../x")).unwrap(), cwd.join("x"));
    }

    #[test]
    fn the_path_is_shown_escaped() {
        let e = verify_copied(&[entry("a\u{1}b", "1")], &[entry("a\u{1}b", "2")]).unwrap_err();
        assert_eq!(e.to_string(), "a\\u{1}b changed while the bundle was built");
    }
}
