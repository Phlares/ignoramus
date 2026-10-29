//! The data-driven checks. Each runs on the nodes its path selects.
use super::{Bound, Check, Kind, Rule};
use crate::diag::Diagnostic;
use serde_json::Value;
use std::collections::HashSet;

const NONFINITE: &str = "!nonfinite:";

/// Every violation of the generic rules, in rule-file order, then in key-sorted order within a rule
/// (array elements by index).
pub fn check_generic(spec: &Value, rules: &[Rule]) -> Vec<Diagnostic> {
    let known: HashSet<&str> = rules
        .iter()
        .filter(|r| matches!(r.check, Check::Type(_)))
        .map(|r| r.path.as_str())
        .collect();
    let mut out = Vec::new();
    for rule in rules.iter().filter(|r| applies(r, spec)) {
        for path in violations(rule, spec, &known) {
            out.push(diagnostic(rule, path, rule.problem.clone()));
        }
    }
    out
}

pub(super) fn diagnostic(rule: &Rule, path: String, problem: String) -> Diagnostic {
    Diagnostic {
        code: rule.code.clone(),
        severity: rule.severity,
        path,
        problem,
        instruction: rule.instruction.clone(),
        doc: format!("docs/rules/{}.md", rule.code),
        sources: rule.sources.clone(),
    }
}

pub(super) fn applies(rule: &Rule, spec: &Value) -> bool {
    let is = |key: &str, want: &Option<String>| {
        want.as_ref()
            .is_none_or(|w| spec["study"][key].as_str() == Some(w))
    };
    rule.when
        .as_ref()
        .is_none_or(|w| is("mode", &w.mode) && is("label", &w.label))
}

fn violations(rule: &Rule, spec: &Value, known: &HashSet<&str>) -> Vec<String> {
    match &rule.check {
        Check::Named(_) | Check::Documented => vec![],
        Check::KnownKey => {
            let mut out = Vec::new();
            unknown_keys(spec, "", "", known, &mut out);
            out
        }
        Check::Unique(key) => select(spec, &rule.path)
            .into_iter()
            .filter_map(|(p, v)| Some((p, v?.as_array()?)))
            .flat_map(|(p, items)| {
                let suffix = key.as_ref().map(|k| format!(".{k}")).unwrap_or_default();
                repeats(items, key.as_deref())
                    .into_iter()
                    .map(move |i| format!("{p}[{i}]{suffix}"))
            })
            .collect(),
        check => select(spec, &rule.path)
            .into_iter()
            .filter(|(_, v)| fails(check, *v))
            .map(|(p, _)| p)
            .collect(),
    }
}

/// Indexes of the items whose string (the item, or its `key` field) appeared earlier.
fn repeats(items: &[Value], key: Option<&str>) -> Vec<usize> {
    let mut seen = HashSet::new();
    items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let s = match key {
                None => item.as_str(),
                Some(k) => item.get(k)?.as_str(),
            }?;
            (!seen.insert(s)).then_some(i)
        })
        .collect()
}

/// `v` is `None` for a missing key. Wrongly typed values pass every check but
/// `Type`, so one fault gives one diagnostic.
fn fails(check: &Check, v: Option<&Value>) -> bool {
    match (check, v) {
        (Check::Required, v) => v.is_none(),
        (Check::NonEmpty, None) => true,
        (Check::NonEmpty, Some(Value::String(s))) => s.is_empty(),
        (Check::NonEmpty, Some(Value::Array(a))) => a.is_empty(),
        (Check::Type(k), Some(v)) => !is_kind(*k, v),
        (Check::Enum(allowed), Some(Value::String(s))) => !allowed.contains(s),
        (Check::Range { min, max }, Some(Value::Number(n))) => n
            .as_f64()
            .is_some_and(|x| !above(*min, x) || !below(*max, x)),
        (Check::Format(re), Some(Value::String(s))) => !re.is_match(s),
        (Check::Finite, Some(Value::String(s))) => s.starts_with(NONFINITE),
        _ => false,
    }
}

fn above(min: Bound, x: f64) -> bool {
    match min {
        Bound::Unbounded => true,
        Bound::Inclusive(m) => x >= m,
        Bound::Exclusive(m) => x > m,
    }
}

fn below(max: Bound, x: f64) -> bool {
    match max {
        Bound::Unbounded => true,
        Bound::Inclusive(m) => x <= m,
        Bound::Exclusive(m) => x < m,
    }
}

/// A float field takes integers and the non-finite markers; `Finite` reports the markers.
fn is_kind(k: Kind, v: &Value) -> bool {
    match (k, v) {
        (Kind::String, Value::String(_))
        | (Kind::Bool, Value::Bool(_))
        | (Kind::Array, Value::Array(_))
        | (Kind::Table, Value::Object(_))
        | (Kind::Float, Value::Number(_)) => true,
        (Kind::Int, Value::Number(n)) => n.is_i64() || n.is_u64(),
        (Kind::Float, Value::String(s)) => s.starts_with(NONFINITE),
        _ => false,
    }
}

pub(super) fn join(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}

/// `join` for a key read from the spec. A key that is not 1 to 40 of
/// `a-z 0-9 _ -` shows as `<key>`: a secret pasted as a key must not
/// reach any output. Paths from this function are for display only.
pub(super) fn join_key(prefix: &str, key: &str) -> String {
    join(prefix, if safe_key(key) { key } else { "<key>" })
}

/// A key that is 1 to 40 of `a-z 0-9 _ -` is safe to print.
pub fn safe_key(key: &str) -> bool {
    (1..=40).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Nodes a path selects, as (concrete path, value). `None` marks a missing key.
/// A missing or non-table parent selects nothing: its own rule reports it.
/// `**` selects every value at any depth.
pub(super) fn select<'a>(root: &'a Value, pattern: &str) -> Vec<(String, Option<&'a Value>)> {
    if pattern == "**" {
        let mut out = Vec::new();
        all_nodes(root, "", &mut out);
        return out;
    }
    let mut cur = vec![(String::new(), Some(root))];
    for seg in pattern.split('.') {
        let (name, each) = match seg.strip_suffix("[*]") {
            Some(n) => (n, true),
            None => (seg, false),
        };
        let mut next = Vec::new();
        for (p, v) in cur {
            let Some(Value::Object(m)) = v else { continue };
            let child_path = join(&p, name);
            let child = m.get(name);
            match (each, child) {
                (false, c) => next.push((child_path, c)),
                (true, Some(Value::Array(a))) => {
                    for (i, e) in a.iter().enumerate() {
                        next.push((format!("{child_path}[{i}]"), Some(e)));
                    }
                }
                (true, _) => {}
            }
        }
        cur = next;
    }
    cur
}

fn all_nodes<'a>(v: &'a Value, path: &str, out: &mut Vec<(String, Option<&'a Value>)>) {
    match v {
        Value::Object(m) => {
            for (k, c) in m {
                let p = join_key(path, k);
                out.push((p.clone(), Some(c)));
                all_nodes(c, &p, out);
            }
        }
        Value::Array(a) => {
            for (i, c) in a.iter().enumerate() {
                let p = format!("{path}[{i}]");
                out.push((p.clone(), Some(c)));
                all_nodes(c, &p, out);
            }
        }
        _ => {}
    }
}

/// Keys whose wildcard path has no `type` target. An unknown table is reported
/// once, without its children.
fn unknown_keys(v: &Value, path: &str, wild: &str, known: &HashSet<&str>, out: &mut Vec<String>) {
    let Value::Object(m) = v else { return };
    for (k, c) in m {
        let (p, w) = (join_key(path, k), join(wild, k));
        if !known.contains(w.as_str()) {
            out.push(p);
            continue;
        }
        match c {
            Value::Object(_) => unknown_keys(c, &p, &w, known, out),
            Value::Array(a) => {
                for (i, e) in a.iter().enumerate() {
                    unknown_keys(e, &format!("{p}[{i}]"), &format!("{w}[*]"), known, out);
                }
            }
            _ => {}
        }
    }
}
