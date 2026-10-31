//! Typed rules, loaded from data. `generic` runs the data-driven checks; `named` the cross-field ones.
use crate::diag::Severity;
use crate::stream::{LogName, Next};
use regex::Regex;
use serde::Deserialize;
use std::path::Path;
use std::sync::OnceLock;

mod generic;
mod named;
pub use generic::{check_generic, safe_key};
pub use named::{
    Borrowed, CAP, CheckCtx, Gates, Lookup, Power, StratumInput, Taken, borrowed, check_named,
    estimate_inputs, granted, power, selected_by,
};

/// The records of a box, by study. An `Err` is a log that cannot be read or trusted.
pub trait BoxRecords {
    /// The verified records of the registered study `id`, in order; `None` when no study
    /// with that id is registered in the box.
    fn study(&self, id: &str) -> Result<Option<Vec<crate::records::Record>>, String>;
    /// The slot and level the stream of `log` would give a study that opened a slot now.
    fn stream_next(&self, log: &str) -> Result<Next, String>;
    /// True when slot `slot` of the stream of `log` was opened by study `study` and is closed.
    fn stream_closed(&self, log: &str, study: &str, slot: u64) -> Result<bool, String>;
}

/// An empty box.
pub struct NoRecords;

impl BoxRecords for NoRecords {
    fn study(&self, _: &str) -> Result<Option<Vec<crate::records::Record>>, String> {
        Ok(None)
    }

    /// Each stream is empty.
    fn stream_next(&self, log: &str) -> Result<Next, String> {
        let log = LogName::parse(log).map_err(|e| e.to_string())?;
        crate::stream::next_of(log, &[], &[]).map_err(|e| e.to_string())
    }

    fn stream_closed(&self, _: &str, _: &str, _: u64) -> Result<bool, String> {
        Ok(false)
    }
}

/// `B`, with each stream read once and the answer kept, so every rule of one check sees one
/// slot and level, or one error.
pub struct ReadOnce<B> {
    inner: B,
    streams: std::cell::RefCell<std::collections::BTreeMap<String, Result<Next, String>>>,
}

impl<B> ReadOnce<B> {
    pub fn new(inner: B) -> ReadOnce<B> {
        ReadOnce {
            inner,
            streams: Default::default(),
        }
    }
}

impl<B: BoxRecords> BoxRecords for ReadOnce<B> {
    fn study(&self, id: &str) -> Result<Option<Vec<crate::records::Record>>, String> {
        self.inner.study(id)
    }

    fn stream_next(&self, log: &str) -> Result<Next, String> {
        let mut streams = self.streams.borrow_mut();
        let read = streams
            .entry(log.to_string())
            .or_insert_with(|| self.inner.stream_next(log));
        read.clone()
    }

    fn stream_closed(&self, log: &str, study: &str, slot: u64) -> Result<bool, String> {
        self.inner.stream_closed(log, study, slot)
    }
}

/// True when `id` has the form E-FMT-001 gives `study.id` in the built-in rules.
pub fn is_study_id(id: &str) -> bool {
    has_format("study.id", id)
}

/// True when `value` has the form E-FMT-001 gives the field at `path` in the built-in rules.
fn has_format(path: &str, value: &str) -> bool {
    builtin().iter().any(|r| {
        r.code == "E-FMT-001"
            && r.path == path
            && matches!(&r.check, Check::Format(re) if re.is_match(value))
    })
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub code: String,
    pub severity: Severity,
    /// Dots, `[*]` for array elements and `*` for every key of a table; `**` means any depth.
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
    Format(Regex),
    Finite,
    /// Every key must match a `Type` target.
    KnownKey,
    /// An array repeats a string, or (with a key) repeats one field across its tables.
    Unique(Option<String>),
    /// A cross-field check in Rust; the generic checker skips it.
    Named(String),
    /// A code the CLI raises itself. It checks no spec field; the rule file only documents it.
    Documented,
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
            Check::Unique(_) => "unique".into(),
            Check::Named(n) => format!("named:{n}"),
            Check::Documented => "documented".into(),
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
    /// Any value. The path and everything below it are known keys; a named check owns the
    /// value's shape.
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Bound {
    Unbounded,
    Inclusive(f64),
    Exclusive(f64),
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
    /// The parser's message, with line and column; the rule code when known.
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
    /// `"documented"` marks a code the CLI raises itself; it takes no targets.
    check: Option<String>,
    #[serde(default)]
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
    key: Option<String>,
}

/// Flattens the file to one `Rule` per target; targets of one code share its text.
pub fn load(path: &Path) -> Result<Vec<Rule>, RulesError> {
    parse(&std::fs::read_to_string(path).map_err(RulesError::Io)?)
}

/// The rules in `rules/v0.toml`, compiled once.
pub fn builtin() -> &'static [Rule] {
    static BUILTIN: OnceLock<Vec<Rule>> = OnceLock::new();
    BUILTIN.get_or_init(|| {
        parse(include_str!("../../../../rules/v0.toml")).expect("embedded v0.toml is valid")
    })
}

fn parse(text: &str) -> Result<Vec<Rule>, RulesError> {
    let file: File = toml::from_str(text).map_err(|e| match failing_code(text) {
        Some(code) => RulesError::Toml(format!("rule {code}: {e}")),
        None => RulesError::Toml(e.to_string()),
    })?;
    let mut out = Vec::new();
    for row in file.rule {
        let invalid = |msg: &str| RulesError::Invalid {
            code: row.code.clone(),
            path: String::new(),
            msg: msg.into(),
        };
        let rule = |path: String, check: Check| Rule {
            code: row.code.clone(),
            severity: row.severity,
            path,
            check,
            problem: row.problem.clone(),
            instruction: row.instruction.clone(),
            why: row.why.clone(),
            sources: row.sources.clone(),
            when: row.when.clone(),
        };
        match (row.check.as_deref(), row.target.is_empty()) {
            (Some("documented"), true) => {
                out.push(rule(String::new(), Check::Documented));
                continue;
            }
            (Some("documented"), false) => {
                return Err(invalid("a documented rule takes no target"));
            }
            (Some(other), _) => return Err(invalid(&format!("unknown rule check \"{other}\""))),
            (None, true) => {
                return Err(invalid("a rule needs a target, or check = \"documented\""));
            }
            (None, false) => {}
        }
        for t in row.target {
            let check = parse_check(&t).map_err(|msg| RulesError::Invalid {
                code: row.code.clone(),
                path: t.path.clone(),
                msg,
            })?;
            out.push(rule(t.path, check));
        }
    }
    Ok(out)
}

/// The code of the first rule that does not fit the schema, if the file parses as TOML.
fn failing_code(text: &str) -> Option<String> {
    let table: toml::Table = toml::from_str(text).ok()?;
    let bad = table
        .get("rule")?
        .as_array()?
        .iter()
        .find(|r| Row::deserialize((*r).clone()).is_err())?;
    bad.get("code")?.as_str().map(str::to_string)
}

fn parse_check(t: &Target) -> Result<Check, String> {
    let need = |what: &str| format!("check \"{}\" needs {what}", t.check);
    let (check, param) = match t.check.as_str() {
        "required" => (Check::Required, None),
        "non-empty" => (Check::NonEmpty, None),
        "finite" => (Check::Finite, None),
        "known-key" => (Check::KnownKey, None),
        "unique" => (Check::Unique(t.key.clone()), Some("key")),
        "type" => (
            Check::Type(t.kind.ok_or_else(|| need("kind"))?),
            Some("kind"),
        ),
        "enum" => {
            let values = t.values.clone().ok_or_else(|| need("values"))?;
            (Check::Enum(values), Some("values"))
        }
        "range" => {
            let iv = t.interval.as_deref().ok_or_else(|| need("interval"))?;
            let (min, max) = parse_interval(iv)?;
            (Check::Range { min, max }, Some("interval"))
        }
        "format" => {
            let pattern = t.pattern.as_deref().ok_or_else(|| need("pattern"))?;
            let regex = Regex::new(pattern).map_err(|e| format!("bad pattern: {e}"))?;
            (Check::Format(regex), Some("pattern"))
        }
        other => match other.strip_prefix("named:") {
            Some(n) if named::is_known(n) => (Check::Named(n.to_string()), None),
            Some(n) => return Err(format!("unknown named check \"{n}\"")),
            _ => return Err(format!("unknown check \"{other}\"")),
        },
    };
    let given = [
        ("kind", t.kind.is_some()),
        ("values", t.values.is_some()),
        ("interval", t.interval.is_some()),
        ("pattern", t.pattern.is_some()),
        ("key", t.key.is_some()),
    ];
    for (name, set) in given {
        if set && Some(name) != param {
            return Err(format!("check \"{}\" does not take \"{name}\"", t.check));
        }
    }
    Ok(check)
}

/// Interval notation: `(0, 0.5]`, `[1, inf)`. `inf` and `-inf` mean unbounded.
/// Both brackets are required. An empty interval is an error: `a > b`, or
/// `a == b` unless both ends are inclusive.
fn parse_interval(s: &str) -> Result<(Bound, Bound), String> {
    let bad = || format!("bad interval \"{s}\"");
    let s = s.trim();
    let (open, close) = (s.chars().next(), s.chars().last());
    if s.len() < 2 || !matches!(open, Some('[' | '(')) || !matches!(close, Some(']' | ')')) {
        return Err(bad());
    }
    let (lo, hi) = s[1..s.len() - 1].split_once(',').ok_or_else(bad)?;
    let num = |x: &str| -> Result<Option<f64>, String> {
        match x.trim() {
            "inf" | "-inf" => Ok(None),
            n => match n.parse::<f64>() {
                Ok(v) if v.is_finite() => Ok(Some(v)),
                _ => Err(bad()),
            },
        }
    };
    let (lo, hi) = (num(lo)?, num(hi)?);
    let min = lo.map_or(Bound::Unbounded, |x| match open {
        Some('[') => Bound::Inclusive(x),
        _ => Bound::Exclusive(x),
    });
    let max = hi.map_or(Bound::Unbounded, |x| match close {
        Some(']') => Bound::Inclusive(x),
        _ => Bound::Exclusive(x),
    });
    if let (Some(a), Some(b)) = (lo, hi) {
        let both_inclusive = open == Some('[') && close == Some(']');
        if a > b || (a == b && !both_inclusive) {
            return Err(bad());
        }
    }
    Ok((min, max))
}

/// The E-TYP-001 diagnostic for a spec that passes every rule and still does not fit the typed
/// model. It sits at `spec` and carries no parser text, which could echo a value.
pub fn schema_fault() -> crate::diag::Diagnostic {
    let rule = builtin()
        .iter()
        .find(|r| r.code == "E-TYP-001")
        .expect("E-TYP-001 is in the rule file");
    generic::diagnostic(rule, "spec".into(), rule.problem.clone())
}

/// Defaults, then generic, then named checks. Returns the defaulted spec, the
/// defaults filled, and every diagnostic.
pub fn check_all(
    value: &serde_json::Value,
    rules: &[Rule],
    ctx: &CheckCtx,
) -> (
    serde_json::Value,
    Vec<(String, serde_json::Value)>,
    Vec<crate::diag::Diagnostic>,
) {
    let mut v = value.clone();
    let filled = crate::estimate::apply_defaults(&mut v);
    let mut diags = check_generic(&v, rules);
    diags.extend(check_named(&v, rules, ctx));
    (v, filled, diags)
}
