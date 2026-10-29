//! Spec loading: TOML file to JSON value plus hash.
use crate::canon;
use serde_json::Value;
use std::path::Path;

pub mod model;

#[derive(Debug)]
pub struct RawSpec {
    pub value: Value,
    /// `"sha256:"` followed by the hex digest of the canonical JSON.
    pub hash: String,
}

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    /// Byte offset of the first invalid UTF-8 sequence.
    Utf8 {
        offset: usize,
    },
    /// 1-based line and column of the parse error.
    Toml {
        line: usize,
        col: usize,
        msg: String,
    },
    Canon(canon::CanonError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "cannot read spec: {e}"),
            LoadError::Utf8 { offset } => write!(f, "spec is not UTF-8 at byte {offset}"),
            LoadError::Toml { line, col, msg } => write!(f, "TOML error at {line}:{col}: {msg}"),
            LoadError::Canon(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for LoadError {}

pub fn load(path: &Path) -> Result<RawSpec, LoadError> {
    let bytes = std::fs::read(path).map_err(LoadError::Io)?;
    let text = std::str::from_utf8(&bytes).map_err(|e| LoadError::Utf8 {
        offset: e.valid_up_to(),
    })?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    // A raw CR is illegal in TOML, so this only touches line endings.
    let text = text.replace("\r\n", "\n");
    let table: toml::Table = toml::from_str(&text).map_err(|e| {
        let offset = e.span().map_or(0, |s| s.start).min(text.len());
        let before = text.get(..offset).unwrap_or("");
        LoadError::Toml {
            line: before.matches('\n').count() + 1,
            col: before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1,
            msg: e.message().to_string(),
        }
    })?;
    let value = to_json(toml::Value::Table(table));
    let hash = format!(
        "sha256:{}",
        canon::sha256_hex(&canon::to_canonical(&value).map_err(LoadError::Canon)?)
    );
    Ok(RawSpec { value, hash })
}

/// Integer vs float and the sign of zero are part of the hash (`1` and `1.0` differ).
/// Datetimes hash as written, normalized only by the toml crate's Display.
/// Non-finite floats become `"!nonfinite:…"` strings so loading never fails;
/// the `Finite` check reports them later.
fn to_json(v: toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => Value::from(i),
        toml::Value::Float(f) => match serde_json::Number::from_f64(f) {
            Some(n) => Value::Number(n),
            None if f.is_nan() => Value::String("!nonfinite:nan".into()),
            None if f > 0.0 => Value::String("!nonfinite:inf".into()),
            None => Value::String("!nonfinite:-inf".into()),
        },
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(a) => Value::Array(a.into_iter().map(to_json).collect()),
        toml::Value::Table(t) => {
            Value::Object(t.into_iter().map(|(k, v)| (k, to_json(v))).collect())
        }
    }
}
