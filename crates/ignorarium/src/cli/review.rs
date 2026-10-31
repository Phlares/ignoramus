//! `review`: one Markdown page for a person to read before approving a plan.
//! It reads the spec as plain values, so it also renders a plan that fails check.
use super::check::{self, Assessed};
use super::{Ctx, Done, Run, out};
use crate::diag::{Diagnostic, Severity};
use crate::rules::{self, safe_key};
use serde_json::Value;
use std::path::Path;

fn at(v: &Value, pointer: &str) -> String {
    v.pointer(pointer).map_or_else(|| "(missing)".into(), plain)
}

/// A value as plain text: a table as `key value` pairs, a list in brackets.
fn plain(v: &Value) -> String {
    let join = |parts: Vec<String>| parts.join(", ");
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => format!("[{}]", join(a.iter().map(plain).collect())),
        Value::Object(m) => join(m.iter().map(|(k, x)| format!("{k} {}", plain(x))).collect()),
        other => other.to_string(),
    }
}

/// `<id>: <statement> (rival: <rival>)`, then what the hypothesis tests and how.
fn hypothesis_line(h: &Value) -> String {
    let strata = items(h, "/strata");
    let strata = if strata.is_empty() {
        "pooled".to_string()
    } else {
        strata.iter().map(plain).collect::<Vec<_>>().join(", ")
    };
    let mut parts = vec![
        format!(
            "{}: {} (rival: {})",
            at(h, "/id"),
            at(h, "/statement"),
            at(h, "/rival")
        ),
        format!("outcome: {}", at(h, "/outcome")),
        format!("contrast: {}", at(h, "/contrast")),
        format!("strata: {strata}"),
    ];
    if h.get("claim").is_none() {
        parts.push("claim: none (descriptive)".into());
    }
    let shown = ["claim", "statistic", "power_at", "estimates", "alpha_share"];
    parts.extend(shown.iter().filter_map(|key| {
        let label = key.replace('_', " ");
        Some(format!("{label}: {}", plain(h.get(*key)?)))
    }));
    parts.join("; ")
}

fn items<'a>(v: &'a Value, pointer: &str) -> &'a [Value] {
    v.pointer(pointer)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn bullets(lines: impl IntoIterator<Item = String>) -> String {
    let mut s: String = lines.into_iter().map(|l| format!("- {l}\n")).collect();
    if s.is_empty() {
        s.push_str("- none\n");
    }
    s
}

fn diag_bullet(d: &Diagnostic) -> String {
    format!(
        "{} at `{}`: {} Fix: {} See: {}",
        d.code, d.path, d.problem, d.instruction, d.doc
    )
}

fn problems(diags: &[Diagnostic]) -> String {
    let of = |sev: Severity, title: &str| {
        let list: Vec<String> = diags
            .iter()
            .filter(|d| d.severity == sev)
            .map(diag_bullet)
            .collect();
        if list.is_empty() {
            String::new()
        } else {
            format!("### {title}\n\n{}\n", bullets(list))
        }
    };
    let s = of(Severity::Error, "Errors") + &of(Severity::Warning, "Warnings");
    if s.is_empty() { "None.\n".into() } else { s }
}

/// The spec with every string the secret scan flags replaced by a marker, and every
/// key that is not safe to print renamed `<key>`. Nothing else of the spec is printed.
fn withheld(v: &Value) -> Value {
    match v {
        Value::String(s) if !crate::scan::scan_str(s, crate::scan::builtin()).is_empty() => {
            Value::String("(withheld: E-SEC-001)".into())
        }
        Value::Array(a) => Value::Array(a.iter().map(withheld).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| {
                    let k = if safe_key(k) { k.as_str() } else { "<key>" };
                    (k.to_string(), withheld(x))
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

const NO_ESTIMATE: &str = "Not available: the plan has errors.\n";

/// The root relative to the current folder when it lies inside it, else the full path.
fn shown_root(root: &Path) -> String {
    let rel = std::env::current_dir()
        .ok()
        .and_then(|cwd| root.strip_prefix(cwd).ok().map(Path::to_path_buf));
    match rel {
        Some(r) if r.as_os_str().is_empty() => ".".into(),
        Some(r) => r.display().to_string(),
        None => root.display().to_string(),
    }
}

fn page(a: &Assessed) -> String {
    let v = &withheld(&a.value);
    let est = a.estimate.as_ref();
    let hypotheses = items(v, "/question/hypotheses").iter().map(hypothesis_line);
    let outcomes = items(v, "/outcomes").iter().map(|o| {
        format!(
            "{} ({}, {} is better)",
            at(o, "/id"),
            at(o, "/unit"),
            at(o, "/direction")
        )
    });
    let conditions: Vec<String> = match est {
        Some(e) => check::condition_lines(e),
        None => items(v, "/design/conditions")
            .iter()
            .map(|c| at(c, "/id"))
            .collect(),
    };
    let inputs = items(v, "/inputs").iter().map(|i| {
        format!(
            "`{}`, role {}, sha256 {}, licence {} ({}), source {}",
            at(i, "/path"),
            at(i, "/role"),
            at(i, "/sha256"),
            at(i, "/licence/spdx"),
            at(i, "/licence/type"),
            at(i, "/licence/source")
        )
    });
    let mut s = format!(
        "# Plan review: {}\n\nSpec hash: {}\n\nStudy root: {}\n\n## Plan\n\n",
        at(v, "/study/id"),
        a.hash,
        shown_root(&a.root)
    );
    s.push_str(&bullets([
        format!(
            "Study: {} ({}, template {})",
            at(v, "/study/id"),
            at(v, "/study/mode"),
            at(v, "/study/template")
        ),
        format!("Population: {}", at(v, "/question/population")),
        format!("Intervention: {}", at(v, "/question/intervention")),
        format!("Comparator: {}", at(v, "/question/comparator")),
        format!("Negative control: {}", at(v, "/question/negative_control")),
    ]));
    s.push_str(&format!("\nHypotheses:\n\n{}", bullets(hypotheses)));
    s.push_str(&format!("\nOutcomes:\n\n{}", bullets(outcomes)));
    s.push_str(&format!("\n## Conditions\n\n{}", bullets(conditions)));
    s.push_str("\n## Estimate\n\n");
    s.push_str(&est.map_or(NO_ESTIMATE.into(), |e| bullets(check::figure_lines(e))));
    let level = a.stream.iter().map(check::stream_line);
    s.push_str(&format!("\n## Level\n\n{}", bullets(level)));
    let power = rules::power(&a.value, &a.ctx)
        .into_iter()
        .map(|p| format!("{}: {}", p.hypothesis, p.sentence()));
    s.push_str(&format!("\n## Power\n\n{}", bullets(power)));
    s.push_str(&format!(
        "\n## Claim label\n\n{}",
        bullets([
            format!("Declared: {}", at(v, "/study/label")),
            est.map_or("Earnable: not available".into(), |e| {
                format!("Earnable: {}", e.label_earnable)
            }),
        ])
    ));
    s.push_str(&format!(
        "\n## Defaults\n\n{}",
        bullets(check::default_lines(a))
    ));
    s.push_str(&format!("\n## Warnings\n\n{}", problems(&a.diags)));
    s.push_str(&format!("\n## Inputs and licences\n\n{}", bullets(inputs)));
    s
}

pub fn run(ctx: &Ctx, spec: &Path) -> Run {
    let a = check::assess(ctx, spec)?;
    out(&page(&a))?;
    let code = a.exit_code();
    Ok(Done {
        spec_hash: Some(a.hash),
        ..Done::exit(code)
    })
}
