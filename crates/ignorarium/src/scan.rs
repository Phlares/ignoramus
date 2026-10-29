//! Offline scan of spec strings for secrets and personal data. Findings carry a
//! kind and a path, never the matched text.
use regex::Regex;
use serde::Deserialize;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct Pattern {
    pub kind: String,
    pub regex: Regex,
    /// A match is dropped when any entry matches its text.
    pub allow: Vec<Regex>,
}

#[derive(Debug)]
pub enum ScanError {
    Io(std::io::Error),
    Toml(String),
    BadRegex { kind: String, msg: String },
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanError::Io(e) => write!(f, "cannot read scan patterns: {e}"),
            ScanError::Toml(m) => write!(f, "scan patterns are not valid: {m}"),
            ScanError::BadRegex { kind, msg } => write!(f, "scan pattern {kind}: {msg}"),
        }
    }
}

impl std::error::Error for ScanError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    pattern: Vec<Row>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    kind: String,
    regex: String,
    #[serde(default)]
    allow: Vec<String>,
}

fn parse(text: &str) -> Result<Vec<Pattern>, ScanError> {
    let file: File = toml::from_str(text).map_err(|e| ScanError::Toml(e.to_string()))?;
    file.pattern
        .into_iter()
        .map(|row| {
            let bad = |e: regex::Error| ScanError::BadRegex {
                kind: row.kind.clone(),
                msg: e.to_string(),
            };
            Ok(Pattern {
                regex: Regex::new(&row.regex).map_err(bad)?,
                allow: row
                    .allow
                    .iter()
                    .map(|a| Regex::new(a).map_err(bad))
                    .collect::<Result<_, _>>()?,
                kind: row.kind,
            })
        })
        .collect()
}

pub fn load(path: &Path) -> Result<Vec<Pattern>, ScanError> {
    parse(&std::fs::read_to_string(path).map_err(ScanError::Io)?)
}

/// The patterns in `rules/scan.toml`, compiled once.
pub fn builtin() -> &'static [Pattern] {
    static BUILTIN: OnceLock<Vec<Pattern>> = OnceLock::new();
    BUILTIN.get_or_init(|| {
        parse(include_str!("../../../rules/scan.toml")).expect("embedded scan.toml is valid")
    })
}

/// True when `p` matches `s` outside its allow list. The search resumes at the end
/// of each reported text, so a match never hides the one after it.
fn finds(p: &Pattern, s: &str) -> bool {
    let mut at = 0;
    while let Some(c) = p.regex.captures_at(s, at) {
        let Some(hit) = c.name("hit").or_else(|| c.get(0)) else {
            return false;
        };
        if !p.allow.iter().any(|a| a.is_match(hit.as_str())) {
            return true;
        }
        // An empty hit would not advance.
        at = hit.end().max(at + 1);
        if at > s.len() {
            return false;
        }
    }
    false
}

/// The kinds of pattern that match `s`, once each, in pattern order.
pub fn scan_str<'a>(s: &str, patterns: &'a [Pattern]) -> Vec<&'a str> {
    patterns
        .iter()
        .filter(|p| finds(p, s))
        .map(|p| p.kind.as_str())
        .collect()
}
