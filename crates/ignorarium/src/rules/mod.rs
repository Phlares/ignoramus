//! Typed rules, loaded from data. `generic` runs the data-driven checks.
use crate::diag::Severity;
use regex::Regex;
use serde::Deserialize;
use std::path::Path;

mod generic;
pub use generic::check_generic;

#[derive(Debug, Clone)]
pub struct Rule {
    pub code: String,
    pub severity: Severity,
    /// Dots and `[*]` for array elements; `**` means any depth.
    pub path: String,
    pub check: Check,
    pub problem: String,
    pub instruction: String,
    pub why: String,
    pub sources: Vec<String>,
    pub when: Option<When>,
}

#[derive(Debug, Clone)]
pub enum Check {
    Required,
    /// Present, and not `""` or `[]`.
    NonEmpty,
    Type(Kind),
    Enum(Vec<String>),
    Range {
        min: Bound,
        max: Bound,
    },
    Format(Fmt),
    Finite,
    /// Every key must match a `Type` target.
    KnownKey,
    /// A cross-field check in Rust; the generic checker skips it.
    Named(String),
}

impl Check {
    /// The check's name as written in the rule file.
    pub fn name(&self) -> String {
        match self {
            Check::Required => "required".into(),
            Check::NonEmpty => "non-empty".into(),
            Check::Type(_) => "type".into(),
            Check::Enum(_) => "enum".into(),
            Check::Range { .. } => "range".into(),
            Check::Format(_) => "format".into(),
            Check::Finite => "finite".into(),
            Check::KnownKey => "known-key".into(),
            Check::Named(n) => format!("named:{n}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    String,
    Int,
    Float,
    Bool,
    Array,
    Table,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Bound {
    Unbounded,
    Inclusive(f64),
    Exclusive(f64),
}

#[derive(Debug, Clone)]
pub struct Fmt {
    pub pattern: String,
    pub regex: Regex,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct When {
    pub mode: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug)]
pub enum RulesError {
    Io(std::io::Error),
    Toml(String),
    Invalid {
        code: String,
        path: String,
        msg: String,
    },
}

impl std::fmt::Display for RulesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RulesError::Io(e) => write!(f, "cannot read rules: {e}"),
            RulesError::Toml(m) => write!(f, "rules file is not valid: {m}"),
            RulesError::Invalid { code, path, msg } => write!(f, "rule {code} at {path}: {msg}"),
        }
    }
}

impl std::error::Error for RulesError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    rule: Vec<Row>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    code: String,
    severity: Severity,
    problem: String,
    instruction: String,
    why: String,
    sources: Vec<String>,
    when: Option<When>,
    target: Vec<Target>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    path: String,
    check: String,
    kind: Option<Kind>,
    values: Option<Vec<String>>,
    interval: Option<String>,
    pattern: Option<String>,
}

/// Flattens the file to one `Rule` per target; targets of one code share its text.
pub fn load(path: &Path) -> Result<Vec<Rule>, RulesError> {
    let text = std::fs::read_to_string(path).map_err(RulesError::Io)?;
    let file: File =
        toml::from_str(&text).map_err(|e| RulesError::Toml(e.message().to_string()))?;
    let mut out = Vec::new();
    for row in file.rule {
        for t in row.target {
            let check = parse_check(&t).map_err(|msg| RulesError::Invalid {
                code: row.code.clone(),
                path: t.path.clone(),
                msg,
            })?;
            out.push(Rule {
                code: row.code.clone(),
                severity: row.severity,
                path: t.path,
                check,
                problem: row.problem.clone(),
                instruction: row.instruction.clone(),
                why: row.why.clone(),
                sources: row.sources.clone(),
                when: row.when.clone(),
            });
        }
    }
    Ok(out)
}

fn parse_check(t: &Target) -> Result<Check, String> {
    let need = |what: &str| format!("check \"{}\" needs {what}", t.check);
    Ok(match t.check.as_str() {
        "required" => Check::Required,
        "non-empty" => Check::NonEmpty,
        "finite" => Check::Finite,
        "known-key" => Check::KnownKey,
        "type" => Check::Type(t.kind.ok_or_else(|| need("kind"))?),
        "enum" => Check::Enum(t.values.clone().ok_or_else(|| need("values"))?),
        "range" => {
            let iv = t.interval.as_deref().ok_or_else(|| need("interval"))?;
            let (min, max) = parse_interval(iv)?;
            Check::Range { min, max }
        }
        "format" => {
            let pattern = t.pattern.clone().ok_or_else(|| need("pattern"))?;
            let regex = Regex::new(&pattern).map_err(|e| format!("bad pattern: {e}"))?;
            Check::Format(Fmt { pattern, regex })
        }
        other => match other.strip_prefix("named:") {
            Some(n) if !n.is_empty() => Check::Named(n.to_string()),
            _ => return Err(format!("unknown check \"{other}\"")),
        },
    })
}

/// Interval notation: `(0, 0.5]`, `[1, inf)`. `inf` and `-inf` mean unbounded.
fn parse_interval(s: &str) -> Result<(Bound, Bound), String> {
    let bad = || format!("bad interval \"{s}\"");
    let s = s.trim();
    let inner = s.get(1..s.len().saturating_sub(1)).ok_or_else(bad)?;
    let (lo, hi) = inner.split_once(',').ok_or_else(bad)?;
    let num = |x: &str| -> Result<Option<f64>, String> {
        match x.trim() {
            "inf" | "-inf" => Ok(None),
            n => n.parse::<f64>().map(Some).map_err(|_| bad()),
        }
    };
    let (lo, hi) = (num(lo)?, num(hi)?);
    let min = match (s.chars().next(), lo) {
        (_, None) => Bound::Unbounded,
        (Some('['), Some(x)) => Bound::Inclusive(x),
        (Some('('), Some(x)) => Bound::Exclusive(x),
        _ => return Err(bad()),
    };
    let max = match (s.chars().last(), hi) {
        (_, None) => Bound::Unbounded,
        (Some(']'), Some(x)) => Bound::Inclusive(x),
        (Some(')'), Some(x)) => Bound::Exclusive(x),
        _ => return Err(bad()),
    };
    if let (Some(a), Some(b)) = (lo, hi)
        && a > b
    {
        return Err(bad());
    }
    Ok((min, max))
}
