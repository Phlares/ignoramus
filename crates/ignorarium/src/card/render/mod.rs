//! The card as Markdown (`md`), as LaTeX fragments (`tex`) and as one figure per hypothesis
//! (`svg`), with the formatting they share. Each function reads the card and nothing else.
//! Display rounding lives here only.
mod md;
mod svg;
mod tex;

pub use md::markdown;
pub use svg::{figure, figures};
pub use tex::{latex, tex_escape};

use super::sections::{Field, Outside, ResultRow};
use super::{Card, DeviationRow, Interval};
use crate::spec::model::Claim;
use serde::Serialize;
use serde_json::Value;

/// The text of a serialized value: a string as it is, anything else as compact JSON.
fn word<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(Value::String(s)) => s,
        Ok(v) => v.to_string(),
        Err(_) => String::new(),
    }
}

/// `x` to 4 significant digits, cut to an integer count of units by `cut`.
fn sig(x: f64, cut: fn(f64) -> f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return x.to_string();
    }
    let e = x.abs().log10().floor() as i32;
    let scale = 10f64.powi(e - 3);
    format!("{:.*}", (3 - e).max(0) as usize, cut(x / scale) * scale)
}

fn sig4(x: f64) -> String {
    sig(x, f64::round)
}

/// An e-value rounds down: the printed number never reaches a threshold the value misses.
fn e_text(x: f64) -> String {
    sig(x, |v| (v + 1e-9).floor())
}

/// A threshold rounds up, so an e-value below it never prints equal to it.
fn threshold_text(x: f64) -> String {
    sig(x, |v| (v - 1e-9).ceil())
}

/// A fraction as a percent with 2 decimals; a change carries its sign.
fn pct(x: f64, signed: bool) -> String {
    if signed {
        format!("{:+.2}%", x * 100.0)
    } else {
        format!("{:.2}%", x * 100.0)
    }
}

/// The decimals in a number's shortest form: 1e-4 gives 4, 0.25 gives 2, 0.5 gives 1.
fn decimals(x: f64) -> usize {
    (x.to_string().split_once('.')).map_or(0, |(_, d)| d.len())
}

/// A bound rounds outward to the decimals of its grid, the margin plus whole steps: up for
/// `below`, down for `above`. The shown number never lies on the passing side of the true bound.
fn bound_text(i: &Interval) -> Option<String> {
    let d = decimals(i.grid.step).max(i.margin.map_or(0, decimals));
    let k = 10f64.powi(d as i32);
    i.bound.map(|b| {
        let n = if i.side == "above" {
            (b * k + 1e-9).floor()
        } else {
            (b * k - 1e-9).ceil()
        };
        format!("{:.*}", d, n / k)
    })
}

/// `x` to 10 decimals with trailing zeros cut: a grid end without float noise.
fn grid_end(x: f64) -> String {
    let s = format!("{x:.10}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn interval_text(i: &Interval) -> String {
    let said = match (bound_text(i), i.contiguous) {
        (Some(b), _) => format!("{} {b}", i.side),
        (None, false) => format!("{}: no interval: the passing margins leave a gap", i.side),
        (None, true) => format!("{}: no grid margin passes", i.side),
    };
    let tag = if i.registered {
        "registered"
    } else {
        "post hoc"
    };
    let g = &i.grid;
    format!(
        "{said} ({}; grid: steps of {} from the registered margin, searched from {} to {}; {tag})",
        i.method,
        g.step,
        grid_end(g.from),
        grid_end(g.to)
    )
}

/// Every `Field` prints through here.
fn field<T>(f: &Field<T>, show: impl FnOnce(&T) -> String) -> String {
    let s = f.shown(show);
    match f {
        Field::Missing {
            link: Some(Outside { path, .. }),
        } => format!("{s} (see {path})"),
        _ => s,
    }
}

fn group(stratum: &str) -> &str {
    if stratum.is_empty() { "all" } else { stratum }
}

fn claim_text(c: &Claim) -> String {
    match c {
        Claim::Below(m) => format!("below {m}"),
        Claim::Above(m) => format!("above {m}"),
        Claim::Within([a, b]) => format!("within {a} to {b}"),
    }
}

fn margins(c: &Claim) -> Vec<f64> {
    match *c {
        Claim::Below(m) | Claim::Above(m) => vec![m],
        Claim::Within([a, b]) => vec![a, b],
    }
}

/// The result rows a hypothesis's claim holds: its strata, or the pooled group.
fn rows_of<'a>(c: &'a Card, h: &str) -> Vec<&'a ResultRow> {
    let strata = (c.claims.iter())
        .find(|l| l.hypothesis == h)
        .map(|l| &l.strata);
    (c.results.iter())
        .filter(|r| r.hypothesis == h)
        .filter(|r| strata.is_none_or(|s| s.is_empty() || s.contains(&r.stratum)))
        .collect()
}

fn alt_text(c: &Card, h: &str) -> String {
    let rows: Vec<String> = (rows_of(c, h).into_iter())
        .map(|r| {
            let mut s = format!("{}: estimate {}", group(&r.stratum), sig4(r.estimate));
            s += &(r.percent_change).map_or_else(String::new, |p| format!(" ({})", pct(p, true)));
            for i in &r.intervals {
                s += &bound_text(i).map_or_else(String::new, |b| format!(", {} bound {b}", i.side));
            }
            s
        })
        .collect();
    let claim = (c.claims.iter())
        .find(|l| l.hypothesis == h)
        .and_then(|l| l.claim);
    let margin = claim.map_or_else(|| "none".into(), |m| claim_text(&m));
    format!("{h}: {}. Registered margin: {margin}.", rows.join("; "))
}

/// Text lines cannot break a table row.
fn flat(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

struct Table {
    head: &'static [&'static str],
    rows: Vec<Vec<String>>,
}

fn results_table(c: &Card) -> Table {
    let rows = (c.results.iter())
        .map(|r| {
            let intervals: Vec<String> = r.intervals.iter().map(interval_text).collect();
            let interval = match (&r.note, intervals.is_empty()) {
                (_, false) => intervals.join("; "),
                (Some(note), true) => note.clone(),
                (None, true) => "no interval".into(),
            };
            vec![
                r.hypothesis.clone(),
                group(&r.stratum).into(),
                r.outcome.clone(),
                sig4(r.estimate),
                r.percent_change.map_or_else(String::new, |p| pct(p, true)),
                interval,
                r.e_value.map_or_else(String::new, e_text),
                r.clusters.to_string(),
                r.units.to_string(),
                match (r.registered, r.descriptive) {
                    (false, _) => "post hoc",
                    (true, true) => "descriptive",
                    (true, false) => "registered",
                }
                .into(),
            ]
        })
        .collect();
    let head = &[
        "Hypothesis",
        "Group",
        "Outcome",
        "Estimate",
        "Change",
        "Interval",
        "E-value",
        "Clusters",
        "Units",
        "Status",
    ];
    Table { head, rows }
}

fn sensitivity_table(c: &Card) -> Table {
    let at = |p: &Option<crate::rules::CurvePoint>| {
        p.as_ref().map_or_else(
            || "past the curve".into(),
            |p| format!("mean {}, power {:.2}", sig4(p.mean), p.power),
        )
    };
    let placed = c
        .sensitivity
        .iter()
        .flat_map(|s| s.placed.iter().map(move |p| (s, p)));
    let rows = placed
        .map(|(s, p)| {
            vec![
                s.hypothesis.clone(),
                group(&p.stratum).into(),
                sig4(p.estimate),
                p.margin.to_string(),
                at(&p.lower),
                at(&p.upper),
            ]
        })
        .collect();
    let head = &[
        "Hypothesis",
        "Group",
        "Estimate",
        "Margin",
        "Nearest below",
        "Nearest above",
    ];
    Table { head, rows }
}

fn deviations_table(c: &Card) -> Table {
    let opt = |s: &Option<String>| s.clone().unwrap_or_default();
    let rows = (c.deviations.iter())
        .map(|d| match d {
            DeviationRow::Signed {
                from,
                reason,
                timing,
                differs,
            } => vec![
                "signed".into(),
                differs.join(", "),
                reason.clone(),
                timing.clone(),
                format!("pin from {from}"),
                String::new(),
                String::new(),
                "register record".into(),
            ],
            DeviationRow::Recorded {
                kind,
                reason,
                timing,
                original,
                justification,
                impact,
                ..
            } => vec![
                "recorded".into(),
                kind.clone(),
                reason.clone(),
                word(timing),
                original.clone(),
                justification.clone(),
                impact.clone(),
                crate::deviations::RECORDED.into(),
            ],
            DeviationRow::Unsigned {
                entry,
                subject,
                reason,
                timing,
                original,
                justification,
                impact,
                source,
                ..
            } => vec![
                format!("unsigned {entry}"),
                subject.clone(),
                reason.clone(),
                word(timing),
                opt(original),
                opt(justification),
                opt(impact),
                source.clone(),
            ],
        })
        .collect();
    let head = &[
        "Mark",
        "Type",
        "Reason",
        "Timing",
        "Original",
        "Justification",
        "Impact",
        "Source",
    ];
    Table { head, rows }
}

/// `s` as a file-name part: lower case, `[a-z0-9-]` only. A name is never empty, so the empty
/// slug is the pooled group's alone.
fn slug(s: &str) -> String {
    (s.to_lowercase().chars())
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}
