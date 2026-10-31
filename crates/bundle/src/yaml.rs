//! Writes a JSON value as block-style YAML.
//!
//! Every string is double-quoted, so YAML never reads `no` or `1.10` as
//! another type.
use serde_json::Value;

/// Renders `v`. A map prints in the map's order. The output ends with `\n`.
pub fn block(v: &Value) -> String {
    let mut out = String::new();
    match v {
        Value::Object(m) if !m.is_empty() => map(m, 0, &mut out),
        Value::Array(a) if !a.is_empty() => seq(a, 0, &mut out),
        _ => {
            out.push_str(&scalar(v));
            out.push('\n');
        }
    }
    out
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => quote(s),
        Value::Array(_) => "[]".into(),
        Value::Object(_) => "{}".into(),
        other => other.to_string(),
    }
}

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{fffe}' | '\u{ffff}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn key(k: &str) -> String {
    let mut b = k.bytes();
    let bare = b.next().is_some_and(|c| c.is_ascii_lowercase())
        && b.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
    if bare { k.to_string() } else { quote(k) }
}

fn map(m: &serde_json::Map<String, Value>, indent: usize, out: &mut String) {
    for (k, v) in m {
        out.push_str(&" ".repeat(indent));
        out.push_str(&key(k));
        entry(v, indent, out);
    }
}

/// Writes the part of a map entry after its key.
fn entry(v: &Value, indent: usize, out: &mut String) {
    match v {
        Value::Object(m) if !m.is_empty() => {
            out.push_str(":\n");
            map(m, indent + 2, out);
        }
        Value::Array(a) if !a.is_empty() => {
            out.push_str(":\n");
            seq(a, indent + 2, out);
        }
        _ => {
            out.push_str(": ");
            out.push_str(&scalar(v));
            out.push('\n');
        }
    }
}

fn seq(a: &[Value], indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    for v in a {
        out.push_str(&pad);
        match v {
            Value::Object(m) if !m.is_empty() => {
                let mut inner = String::new();
                map(m, indent + 2, &mut inner);
                out.push_str("- ");
                out.push_str(&inner[indent + 2..]);
            }
            Value::Array(a) if !a.is_empty() => {
                out.push_str("-\n");
                seq(a, indent + 2, out);
            }
            _ => {
                out.push_str("- ");
                out.push_str(&scalar(v));
                out.push('\n');
            }
        }
    }
}
