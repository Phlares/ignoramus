//! Reads `citation.toml` and renders CITATION.cff and DataCite metadata from it.
//!
//! The keys of `citation.toml` are the Citation File Format 1.2.0 keys.
use crate::tree::shown;
use crate::{Problem, blank};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::path::Path;

const CITATION_INSTRUCTION: &str = "Fix citation.toml; its keys are the Citation File Format 1.2.0 keys (https://github.com/citation-file-format/citation-file-format/blob/main/schema-guide.md).";
const RELEASE_INSTRUCTION: &str = "Use a version of letters, digits, dots and hyphens, a YYYY-MM-DD date, a 40-character lowercase hex commit, a DOI that starts with 10. and a publisher name.";

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Citation {
    pub title: String,
    pub message: String,
    #[serde(rename = "abstract")]
    pub summary: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub authors: Vec<Author>,
    #[serde(default)]
    pub references: Vec<Reference>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Author {
    pub family_names: Option<String>,
    pub given_names: Option<String>,
    pub name: Option<String>,
    pub orcid: Option<String>,
    pub affiliation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Reference {
    #[serde(rename = "type")]
    pub kind: RefKind,
    pub title: String,
    pub authors: Vec<Author>,
    pub doi: Option<String>,
    pub url: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefKind {
    Article,
    Data,
    Software,
}

impl RefKind {
    fn as_str(self) -> &'static str {
        match self {
            RefKind::Article => "article",
            RefKind::Data => "data",
            RefKind::Software => "software",
        }
    }
}

fn problem(path: &str, message: impl Into<String>) -> Problem {
    Problem::new(
        "E-PUB-004",
        format!("citation.toml{path}"),
        message,
        CITATION_INSTRUCTION,
    )
}

fn set(o: &Option<String>) -> bool {
    o.as_deref().is_some_and(|s| !blank(s))
}

/// True for `10.` plus 4 to 9 digits, `/`, and a suffix; no whitespace.
pub fn is_doi(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("10.") else {
        return false;
    };
    let Some((digits, suffix)) = rest.split_once('/') else {
        return false;
    };
    (4..=9).contains(&digits.len())
        && digits.bytes().all(|b| b.is_ascii_digit())
        && !suffix.is_empty()
        && !s.chars().any(char::is_whitespace)
}

fn is_orcid(s: &str) -> bool {
    let Some(id) = s.strip_prefix("https://orcid.org/") else {
        return false;
    };
    let b = id.as_bytes();
    b.len() == 19
        && b.iter().enumerate().all(|(i, &c)| match i {
            4 | 9 | 14 => c == b'-',
            18 => c.is_ascii_digit() || c == b'X',
            _ => c.is_ascii_digit(),
        })
}

fn check_author(a: &Author, path: &str, out: &mut Vec<Problem>) {
    let person = set(&a.family_names) && set(&a.given_names) && a.name.is_none();
    let entity = set(&a.name) && a.family_names.is_none() && a.given_names.is_none();
    if !(person || entity) {
        out.push(problem(
            path,
            "an author needs family-names and given-names, or a name alone",
        ));
    }
    if a.orcid.as_deref().is_some_and(|o| !is_orcid(o)) {
        out.push(problem(
            &format!("{path}.orcid"),
            "an orcid is https://orcid.org/ and four groups of four digits; the last character may be X",
        ));
    }
}

fn check_authors(authors: &[Author], path: &str, out: &mut Vec<Problem>) {
    if authors.is_empty() {
        out.push(problem(path, "at least one author is needed"));
    }
    for (i, a) in authors.iter().enumerate() {
        check_author(a, &format!("{path}[{i}]"), out);
    }
}

fn validate(c: &Citation) -> Vec<Problem> {
    let mut out = Vec::new();
    for (path, text) in [
        (":title", &c.title),
        (":message", &c.message),
        (":abstract", &c.summary),
    ] {
        if blank(text) {
            out.push(problem(path, "this field is blank"));
        }
    }
    check_authors(&c.authors, ":authors", &mut out);
    for (i, r) in c.references.iter().enumerate() {
        let at = format!(":references[{i}]");
        if blank(&r.title) {
            out.push(problem(&format!("{at}.title"), "this field is blank"));
        }
        check_authors(&r.authors, &format!("{at}.authors"), &mut out);
        if r.doi.is_none() && r.url.is_none() {
            out.push(problem(&at, "a reference needs a doi or a url"));
        }
        if r.doi.as_deref().is_some_and(|d| !is_doi(d)) {
            out.push(problem(
                &format!("{at}.doi"),
                "a doi starts with 10. and has a prefix and a suffix",
            ));
        }
        if r.url
            .as_deref()
            .is_some_and(|u| !(u.starts_with("https://") || u.starts_with("http://")))
        {
            out.push(problem(
                &format!("{at}.url"),
                "a url starts with https:// or http://",
            ));
        }
    }
    out
}

/// Parses and checks `citation.toml`. Every problem is returned at once.
pub fn parse(text: &str) -> Result<Citation, Vec<Problem>> {
    let c: Citation = toml::from_str(text).map_err(|e| {
        vec![problem(
            "",
            format!("cannot parse: {}", shown(&e.to_string())),
        )]
    })?;
    let problems = validate(&c);
    if problems.is_empty() {
        Ok(c)
    } else {
        Err(problems)
    }
}

pub fn load(path: &Path) -> Result<Citation, Vec<Problem>> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        vec![problem(
            "",
            format!("cannot read: {}", shown(&e.to_string())),
        )]
    })?;
    parse(&text)
}

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub date: String,
    pub commit: Option<String>,
    pub doi: Option<String>,
    pub publisher: String,
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    let num = |from: usize, to: usize| -> Option<u32> {
        let part = b.get(from..to)?;
        part.iter()
            .all(u8::is_ascii_digit)
            .then(|| part.iter().fold(0, |n, d| n * 10 + u32::from(d - b'0')))
    };
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && num(0, 4).is_some()
        && num(5, 7).is_some_and(|m| (1..=12).contains(&m))
        && num(8, 10).is_some_and(|d| (1..=31).contains(&d))
}

/// Checks the release facts the caller passes in.
pub fn check_release(r: &Release) -> Vec<Problem> {
    let bad = |path: &str, message: &str| {
        Problem::new(
            "E-PUB-013",
            format!("release.{path}"),
            message,
            RELEASE_INSTRUCTION,
        )
    };
    let mut out = Vec::new();
    if r.version.is_empty()
        || !r
            .version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        out.push(bad(
            "version",
            "the version is empty or has a character outside letters, digits, dots and hyphens",
        ));
    }
    if !is_date(&r.date) {
        out.push(bad("date", "the date is not a YYYY-MM-DD date"));
    }
    if r.commit.as_deref().is_some_and(|c| {
        c.len() != 40 || !c.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    }) {
        out.push(bad(
            "commit",
            "the commit is not 40 lowercase hex characters",
        ));
    }
    if r.doi.as_deref().is_some_and(|d| !is_doi(d)) {
        out.push(bad(
            "doi",
            "the DOI does not start with 10. or has no valid prefix and suffix",
        ));
    }
    if blank(&r.publisher) {
        out.push(bad("publisher", "the publisher is blank"));
    }
    out
}

#[derive(Debug, Clone)]
pub struct Tool {
    pub name: String,
    pub version: String,
    pub url: String,
    pub author: String,
}

fn put(m: &mut Map<String, Value>, k: &str, v: &Option<String>) {
    if let Some(v) = v {
        m.insert(k.into(), json!(v));
    }
}

fn cff_author(a: &Author) -> Value {
    let mut m = Map::new();
    put(&mut m, "family-names", &a.family_names);
    put(&mut m, "given-names", &a.given_names);
    put(&mut m, "name", &a.name);
    put(&mut m, "orcid", &a.orcid);
    put(&mut m, "affiliation", &a.affiliation);
    Value::Object(m)
}

fn cff_reference(r: &Reference) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), json!(r.kind.as_str()));
    m.insert("title".into(), json!(r.title));
    m.insert(
        "authors".into(),
        Value::Array(r.authors.iter().map(cff_author).collect()),
    );
    put(&mut m, "doi", &r.doi);
    put(&mut m, "url", &r.url);
    put(&mut m, "version", &r.version);
    Value::Object(m)
}

/// The CITATION.cff tree. Serialise it with `yaml::block`.
pub fn cff(c: &Citation, r: &Release, t: &Tool, data_licence: &str) -> Value {
    let mut m = Map::new();
    m.insert("cff-version".into(), json!("1.2.0"));
    m.insert("type".into(), json!("dataset"));
    m.insert("title".into(), json!(c.title));
    m.insert("message".into(), json!(c.message));
    m.insert("abstract".into(), json!(c.summary));
    m.insert(
        "authors".into(),
        Value::Array(c.authors.iter().map(cff_author).collect()),
    );
    if !c.keywords.is_empty() {
        m.insert("keywords".into(), json!(c.keywords));
    }
    m.insert("version".into(), json!(r.version));
    m.insert("date-released".into(), json!(r.date));
    put(&mut m, "commit", &r.commit);
    put(&mut m, "doi", &r.doi);
    m.insert("license".into(), json!(data_licence));
    let tool = Reference {
        kind: RefKind::Software,
        title: t.name.clone(),
        authors: vec![Author {
            family_names: None,
            given_names: None,
            name: Some(t.author.clone()),
            orcid: None,
            affiliation: None,
        }],
        doi: None,
        url: Some(t.url.clone()),
        version: Some(t.version.clone()),
    };
    let refs = std::iter::once(cff_reference(&tool)).chain(c.references.iter().map(cff_reference));
    m.insert("references".into(), Value::Array(refs.collect()));
    Value::Object(m)
}

fn datacite_creator(a: &Author) -> Value {
    let (Some(family), Some(given)) = (&a.family_names, &a.given_names) else {
        return json!({"name": a.name, "nameType": "Organizational"});
    };
    let mut m = Map::new();
    m.insert("name".into(), json!(format!("{family}, {given}")));
    m.insert("nameType".into(), json!("Personal"));
    m.insert("familyName".into(), json!(family));
    m.insert("givenName".into(), json!(given));
    if let Some(o) = &a.orcid {
        m.insert(
            "nameIdentifiers".into(),
            json!([{"nameIdentifier": o, "nameIdentifierScheme": "ORCID", "schemeUri": "https://orcid.org"}]),
        );
    }
    if let Some(aff) = &a.affiliation {
        m.insert("affiliation".into(), json!([{"name": aff}]));
    }
    Value::Object(m)
}

fn related(id: &str, kind: &str) -> Value {
    json!({"relatedIdentifier": id, "relatedIdentifierType": kind, "relationType": "References"})
}

/// The DataCite metadata (kernel 4) for the same release.
pub fn datacite(c: &Citation, r: &Release, t: &Tool, data_licence: &str) -> Value {
    let year = r
        .date
        .get(..4)
        .filter(|y| y.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|y| y.parse::<u32>().ok())
        .unwrap_or(0);
    let mut ids = vec![related(&t.url, "URL")];
    for x in &c.references {
        match (&x.doi, &x.url) {
            (Some(d), _) => ids.push(related(d, "DOI")),
            (None, Some(u)) => ids.push(related(u, "URL")),
            (None, None) => {}
        }
    }
    let mut m = Map::new();
    m.insert(
        "schemaVersion".into(),
        json!("http://datacite.org/schema/kernel-4"),
    );
    m.insert("titles".into(), json!([{"title": c.title}]));
    m.insert(
        "creators".into(),
        Value::Array(c.authors.iter().map(datacite_creator).collect()),
    );
    m.insert("publisher".into(), json!({"name": r.publisher}));
    m.insert("publicationYear".into(), json!(year));
    m.insert(
        "types".into(),
        json!({"resourceType": "Study bundle", "resourceTypeGeneral": "Dataset"}),
    );
    m.insert("version".into(), json!(r.version));
    m.insert(
        "dates".into(),
        json!([{"date": r.date, "dateType": "Issued"}]),
    );
    m.insert(
        "descriptions".into(),
        json!([{"description": c.summary, "descriptionType": "Abstract"}]),
    );
    if !c.keywords.is_empty() {
        let subjects: Vec<Value> = c.keywords.iter().map(|k| json!({"subject": k})).collect();
        m.insert("subjects".into(), Value::Array(subjects));
    }
    put(&mut m, "doi", &r.doi);
    m.insert("relatedIdentifiers".into(), Value::Array(ids));
    m.insert(
        "rightsList".into(),
        json!([{
            "rights": data_licence,
            "rightsIdentifier": data_licence.to_lowercase(),
            "rightsIdentifierScheme": "SPDX",
            "rightsUri": format!("https://spdx.org/licenses/{data_licence}.html"),
            "schemeUri": "https://spdx.org/licenses/",
        }]),
    );
    Value::Object(m)
}
