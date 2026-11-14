//! The card as Markdown, as LaTeX fragments and as one figure per hypothesis.
//! Each function reads the card and nothing else. Display rounding lives here only.
use super::sections::{Field, Outside, ResultRow, UNRECORDED};
use super::{Card, DeviationRow, Interval};
use crate::spec::model::{Claim, StatisticSpec};
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

/// A fraction as a percent with 2 decimals; a change carries its sign.
fn pct(x: f64, signed: bool) -> String {
    if signed {
        format!("{:+.2}%", x * 100.0)
    } else {
        format!("{:.2}%", x * 100.0)
    }
}

/// The decimals in a grid step's shortest form: 1e-4 gives 4, 0.25 gives 2, 0.5 gives 1.
fn decimals(step: f64) -> usize {
    (step.to_string().split_once('.')).map_or(0, |(_, d)| d.len())
}

/// A bound rounds outward to the step's decimals: up for `below`, down for `above`. The shown
/// number never lies on the passing side of the true bound.
fn bound_text(i: &Interval) -> Option<String> {
    let d = decimals(i.grid.step);
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
                if r.registered {
                    "registered"
                } else {
                    "post hoc"
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
                "deviations.jsonl".into(),
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

fn md_table(t: &Table) -> String {
    let line = |cells: Vec<String>| format!("| {} |\n", cells.join(" | "));
    let mut s = line(t.head.iter().map(|h| h.to_string()).collect());
    s += &line(t.head.iter().map(|_| "---".to_string()).collect());
    for r in &t.rows {
        s += &line(r.iter().map(|c| esc(c)).collect());
    }
    s
}

/// `\ { } $ & # ^ _ % ~` made safe in LaTeX text.
pub fn tex_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\\' => "\\textbackslash{}".into(),
            '^' => "\\textasciicircum{}".into(),
            '~' => "\\textasciitilde{}".into(),
            '{' | '}' | '$' | '&' | '#' | '_' | '%' => format!("\\{c}"),
            c => c.to_string(),
        })
        .collect()
}

fn tabular(t: &Table) -> String {
    let row = |cells: Vec<String>| format!("{} \\\\ \\hline\n", cells.join(" & "));
    let spec = vec!["l"; t.head.len()].join(" ");
    let mut s = format!("\\begin{{tabular}}{{{spec}}}\n\\hline\n");
    s += &row(t.head.iter().map(|h| tex_escape(h)).collect());
    for r in &t.rows {
        s += &row(r.iter().map(|c| tex_escape(&flat(c))).collect());
    }
    s + "\\end{tabular}\n"
}

/// `s` as a file-name part: lower case, `[a-z0-9-]` only. A name is never empty, so the empty
/// slug is the pooled group's alone.
fn slug(s: &str) -> String {
    (s.to_lowercase().chars())
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The inline values of one result row: field name and text.
fn values(r: &ResultRow) -> Vec<(String, String)> {
    let mut v = vec![("estimate".to_string(), sig4(r.estimate))];
    v.extend(
        r.percent_change
            .map(|p| ("percent-change".into(), pct(p, true))),
    );
    v.extend(r.e_value.map(|e| ("e-value".into(), e_text(e))));
    for i in &r.intervals {
        v.extend(bound_text(i).map(|b| (format!("bound-{}", slug(&i.side)), b)));
    }
    v
}

/// The LaTeX fragments of the card: (file name, contents). Every table is a plain `tabular`.
/// Two values that name the same file are refused.
pub fn latex(c: &Card) -> Result<Vec<(String, String)>, String> {
    let mut out = vec![
        ("study.tex".to_string(), tex_escape(&c.study)),
        ("results.tex".into(), tabular(&results_table(c))),
        ("deviations.tex".into(), tabular(&deviations_table(c))),
        ("sensitivity.tex".into(), tabular(&sensitivity_table(c))),
    ];
    let mut from = std::collections::BTreeMap::new();
    for r in &c.results {
        for (name, text) in values(r) {
            let (h, g) = (slug(&r.hypothesis), slug(&r.stratum));
            let file = format!("value-{h}-{g}-{name}.tex");
            let source = format!("({:?}, {:?}, {name})", r.hypothesis, r.stratum);
            if let Some(first) = from.insert(file.clone(), source.clone()) {
                return Err(format!(
                    "{file}: two values (hypothesis, group, field) share it: {first} and {source}"
                ));
            }
            out.push((file, tex_escape(&text)));
        }
    }
    Ok(out)
}

const INK: &str = "#1a1a1a";
const POINT: &str = "#1f5fa8";
const MARGIN: &str = "#b3261e";

/// The file the Markdown links for the figure of hypothesis `h`.
fn figure_file(h: &str) -> String {
    format!("figure-{}.svg", slug(h))
}

/// One figure file per claim: (file name, SVG). Two hypotheses that name the same file are refused.
pub fn figures(c: &Card) -> Result<Vec<(String, String)>, String> {
    let mut from = std::collections::BTreeMap::new();
    let mut out = vec![];
    for l in &c.claims {
        let file = figure_file(&l.hypothesis);
        if let Some(first) = from.insert(file.clone(), &l.hypothesis) {
            return Err(format!(
                "{file}: two hypotheses share it: {first:?} and {:?}",
                l.hypothesis
            ));
        }
        out.push((file, figure(c, &l.hypothesis)));
    }
    Ok(out)
}

fn xml(s: &str) -> String {
    (s.replace('&', "&amp;"))
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The figure of hypothesis `h`: each group's estimate as a dot, a line to its interval bound,
/// and a dashed line at each registered margin. Light theme, opaque fixed colours.
pub fn figure(c: &Card, h: &str) -> String {
    const WIDTH: f64 = 560.0;
    const LEFT: f64 = 90.0;
    const ROW: f64 = 36.0;
    const TOP: f64 = 28.0;
    let rows = rows_of(c, h);
    let claim = (c.claims.iter())
        .find(|l| l.hypothesis == h)
        .and_then(|l| l.claim);
    let marks = claim.map(|m| margins(&m)).unwrap_or_default();
    let ends = |r: &ResultRow| {
        let bounds = r.intervals.iter().filter_map(|i| i.bound);
        std::iter::once(r.estimate)
            .chain(bounds)
            .collect::<Vec<_>>()
    };
    let all: Vec<f64> = (rows.iter().flat_map(|r| ends(r)))
        .chain(marks.iter().copied())
        .collect();
    let lo = all.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = all.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let (lo, hi) = if lo < hi {
        (lo, hi)
    } else {
        (lo - 1.0, lo + 1.0)
    };
    let pad = (hi - lo) * 0.1;
    let x = |v: f64| LEFT + (v - lo + pad) / (hi - lo + 2.0 * pad) * (WIDTH - LEFT - 20.0);
    let height = TOP + ROW * rows.len() as f64 + 12.0;
    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{WIDTH}\" height=\"{height}\" \
         viewBox=\"0 0 {WIDTH} {height}\" role=\"img\" font-family=\"sans-serif\" font-size=\"12\">\n\
         <title>{}</title>\n<rect width=\"{WIDTH}\" height=\"{height}\" fill=\"#ffffff\"/>\n",
        xml(&alt_text(c, h))
    );
    for m in &marks {
        let at = x(*m);
        s += &format!(
            "<line x1=\"{at:.1}\" y1=\"{}\" x2=\"{at:.1}\" y2=\"{}\" stroke=\"{MARGIN}\" \
             stroke-width=\"1.5\" stroke-dasharray=\"4 3\"/>\n\
             <text x=\"{at:.1}\" y=\"14\" fill=\"{MARGIN}\" text-anchor=\"middle\">margin {m}</text>\n",
            TOP - 8.0,
            height - 8.0
        );
    }
    for (n, r) in rows.iter().enumerate() {
        let y = TOP + ROW * (n as f64 + 0.5);
        s += &format!(
            "<text x=\"8\" y=\"{:.1}\" fill=\"{INK}\">{}</text>\n",
            y + 4.0,
            xml(group(&r.stratum))
        );
        for b in r.intervals.iter().filter_map(|i| i.bound) {
            s += &format!(
                "<line x1=\"{:.1}\" y1=\"{y:.1}\" x2=\"{:.1}\" y2=\"{y:.1}\" stroke=\"{POINT}\" stroke-width=\"2\"/>\n",
                x(r.estimate),
                x(b)
            );
        }
        s += &format!(
            "<circle cx=\"{:.1}\" cy=\"{y:.1}\" r=\"4.5\" fill=\"{POINT}\"/>\n",
            x(r.estimate)
        );
    }
    s + "</svg>\n"
}

/// Card text as inline Markdown text: one line, no markup. Every card string goes through here.
fn esc(s: &str) -> String {
    let mut out = String::new();
    for c in flat(s).chars() {
        if "\\`*_[]<>|&~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `text` in a code fence longer than any run of backticks in it.
fn fence(text: &str, lang: &str) -> String {
    let run = (text.split(|c| c != '`').map(str::len)).max().unwrap_or(0);
    let f = "`".repeat((run + 1).max(3));
    format!("{f}{lang}\n{}\n{f}", text.trim_end())
}

fn list(items: Vec<String>) -> String {
    items
        .into_iter()
        .map(|i| format!("- {}\n", esc(&i)))
        .collect()
}

fn names(v: &[String]) -> String {
    v.join(", ")
}

fn header_section(c: &Card) -> String {
    let h = &c.header;
    let members: Vec<String> = (c.family["members"].as_array().into_iter().flatten())
        .map(|m| {
            let at = |k: &str| m[k].as_str().unwrap_or_default();
            format!("{} ({})", at("study"), at("state"))
        })
        .collect();
    let family = match c.family["root"].as_str() {
        Some(root) if members.is_empty() => format!("root {root}; no other member"),
        Some(root) => format!("root {root}; other members: {}", names(&members)),
        None => "none".into(),
    };
    let levels = |l: &Vec<u8>| l.iter().map(u8::to_string).collect::<Vec<_>>().join(", ");
    list(vec![
        format!("Study: {}", c.study),
        format!("Family: {family}"),
        format!("Mode: {}", word(&h.mode)),
        format!("Blind: {}", if h.blind { "yes" } else { "no" }),
        format!(
            "Isolation level: {}",
            field(&c.setup.stack.isolation, levels)
        ),
        format!(
            "Registered: {} (plan hash {})",
            h.registered_at, h.spec_hash
        ),
        format!("Outside time stamp: {}", field(&h.timestamp, |t| t.clone())),
        format!("Finished: {}", c.finished_at),
        format!("Records verified: {}", c.records),
    ])
}

fn claim_section(c: &Card) -> String {
    list(
        (c.claims.iter())
            .map(|l| {
                let statistic = match l.statistic {
                    Some(StatisticSpec::Mean([lo, hi])) => format!("mean clipped to [{lo}, {hi}]"),
                    Some(StatisticSpec::Quantile(q)) => format!("quantile {q}"),
                    None => "no statistic".into(),
                };
                let groups = if l.strata.is_empty() {
                    "the pooled group".into()
                } else {
                    names(&l.strata)
                };
                let pass = (l.pass_e_value).map_or_else(String::new, |e| {
                    format!("; pass rule: e-value at least {} in {groups}", sig4(e))
                });
                let claim = l
                    .claim
                    .map_or_else(|| "no claim".into(), |m| claim_text(&m));
                let level = l.level.map_or_else(String::new, |v| format!("; level {v}"));
                let said = if l.statement.is_empty() {
                    String::new()
                } else {
                    format!("{}; ", l.statement)
                };
                format!(
                    "{} ({}): {said}claim {claim}; {statistic}{level}{pass}",
                    l.hypothesis, l.outcome
                )
            })
            .collect(),
    )
}

fn verdict_section(c: &Card) -> String {
    let lines = (c.verdicts.iter())
        .map(|v| {
            let h = c.hypotheses.iter().find(|h| h.hypothesis == v.hypothesis);
            let facts: Vec<String> = (h.into_iter())
                .flat_map(|h| {
                    let mut f = vec![format!("basis {}", h.basis)];
                    f.extend(h.level.map(|l| format!("level {l}")));
                    f.extend(h.e_value.map(|e| format!("e-value {}", e_text(e))));
                    f.extend(h.reason.clone());
                    f
                })
                .collect();
            format!("{}: {} ({})", v.hypothesis, v.display, names(&facts))
        })
        .collect();
    let mut s = list(lines);
    if c.verdicts.iter().any(|v| v.display == "not shown") {
        s += "\n\"Not shown\" means the registered test did not reach its pass e-value. \
              It does not show the claim is false.\n";
    }
    s
}

fn sensitivity_section(c: &Card) -> String {
    let lines = (c.sensitivity.iter())
        .flat_map(|s| {
            let head = format!(
                "{}: power {:.2} at {} (target {:.2})",
                s.hypothesis,
                s.power,
                sig4(s.power_at),
                s.target
            );
            let curves = s
                .curve
                .iter()
                .map(|k| format!("{}: {}", s.hypothesis, k.line()));
            std::iter::once(head).chain(curves)
        })
        .collect();
    format!("{}\n{}", list(lines), md_table(&sensitivity_table(c)))
}

fn figures_section(c: &Card) -> String {
    (c.claims.iter())
        .map(|l| {
            let alt = esc(&alt_text(c, &l.hypothesis));
            format!("- ![{alt}](figure-{}.svg)\n", slug(&l.hypothesis))
        })
        .collect()
}

fn setup_section(c: &Card) -> String {
    let (s, k) = (&c.setup, &c.setup.stack);
    let sha = |d: &Option<String>| d.as_deref().unwrap_or("absent").to_string();
    let mut l = vec![];
    l.extend(s.data.iter().map(|(p, d)| format!("Input {p}: {}", sha(d))));
    l.push(format!(
        "Data grade: {}",
        field(&s.data_grade, |g| g.clone())
    ));
    l.push(format!("Payload: {}", s.instrument.payload));
    l.push(format!("Environment: {}", s.instrument.environment));
    l.extend(
        (s.instrument.inputs.iter()).map(|(p, role, d)| format!("Model input {p} ({role}): {d}")),
    );
    l.push(format!(
        "Seed root (plan hash): {}",
        field(&s.seeds.root, |r| r.clone())
    ));
    l.push(format!(
        "Row seeds: {} of {} derive from the root by: {}",
        s.seeds.derived, s.seeds.rows, s.seeds.rule
    ));
    l.push(format!("Launcher: {}", field(&k.launcher, |v| names(v))));
    l.push(format!(
        "Agent SHA-256: {}",
        field(&k.agent_sha256, |v| names(v))
    ));
    l.push(format!("Virtual CPUs: {}", field(&k.vcpus, u32::to_string)));
    l.push(format!(
        "Row memory cap (MiB): {}",
        field(&k.memory_cap_mib, u32::to_string)
    ));
    l.push(format!("Values the box defaulted: {}", s.defaults));
    l.extend(
        (s.resolved.iter())
            .map(|r| format!("Taken from study {}: {} = {}", r.study, r.path, r.value)),
    );
    l.extend((c.outside.files.iter()).map(|f| format!("File {}: {}", f.path, sha(&f.sha256))));
    list(l)
}

fn surprises_section(c: &Card) -> String {
    let s = &c.surprises;
    let mut l = vec![];
    l.extend(s.failed.iter().map(|f| {
        let row = f.row.map_or_else(|| "no row".into(), |r| r.to_string());
        let why = f.reason.clone().unwrap_or_default();
        format!("Failed row {row} at {}: {} {why}", f.stage, f.exit)
    }));
    l.push(format!("Retried attempts: {}", s.retried));
    l.extend(s.clipped.iter().map(|k| {
        let share = (k.below + k.above) as f64 / k.units.max(1) as f64;
        format!(
            "Clipped in {} {}: {} below, {} above, of {} units ({})",
            k.hypothesis,
            group(&k.stratum),
            k.below,
            k.above,
            k.units,
            pct(share, false)
        )
    }));
    l.extend(s.triggers.iter().map(|t| {
        let skipped: Vec<String> = (t.skipped.iter())
            .map(|k| format!("{} {}", k.child, k.reason))
            .collect();
        let fired = if t.fired { "fired" } else { "not fired" };
        let why = t
            .reason
            .clone()
            .map_or_else(String::new, |r| format!("; {r}"));
        format!(
            "Trigger {}: e-value {} against {}, {fired}; admitted {}; skipped {}{why}",
            t.id,
            e_text(t.e_value),
            sig4(t.threshold),
            names(&t.admitted),
            names(&skipped)
        )
    }));
    let drift = if s.box_versions.len() > 1 {
        " (version drift)"
    } else {
        ""
    };
    l.push(format!("Box versions: {}{drift}", names(&s.box_versions)));
    list(l)
}

fn cost_section(c: &Card) -> String {
    let (k, t) = (&c.cost, &c.cost.telemetry);
    let peak = t
        .memory_peak_max
        .map_or_else(|| "none read".into(), |m| m.to_string());
    list(vec![
        format!(
            "CPU seconds: {}; budget {} CPU hours",
            k.cpu_seconds, k.budget.cpu_hours
        ),
        format!(
            "Wall seconds: {}; budget {} wall hours",
            k.wall_seconds, k.budget.wall_hours
        ),
        format!(
            "Money (USD): {}; budget {}",
            field(&k.usd, f64::to_string),
            k.budget.money_usd
        ),
        format!(
            "Completed rows: {}; attempts charged the row estimate for an unread CPU count: {}",
            k.rows, k.unknown_cpu
        ),
        format!("Telemetry: {} of {} rows hold it", t.recorded, t.rows),
        format!(
            "Host CPU models: {} ({} read, {} unavailable)",
            if t.host_cpu.value.is_empty() {
                "none".into()
            } else {
                names(&t.host_cpu.value)
            },
            t.host_cpu.read,
            t.host_cpu.unavailable
        ),
        format!("CPU ticks, seconds: {}", word(&t.cpu_ticks.value)),
        format!("Cgroup counters: {}", word(&t.cgroup.value)),
        format!("Peak memory, bytes: {peak}"),
    ])
}

fn runs_section(c: &Card) -> String {
    let r = &c.runs;
    let mut l: Vec<String> = (r.ok.iter())
        .map(|g| format!("{}: {} rows ok", g.stage, g.rows))
        .collect();
    l.extend(r.attempts.iter().map(|a| {
        let row = a.row.map_or_else(|| "no row".into(), |n| n.to_string());
        let cpu = a
            .cpu_seconds
            .map_or_else(String::new, |s| format!(", {s} CPU seconds"));
        let why = a
            .reason
            .as_ref()
            .map_or_else(String::new, |r| format!(", {r}"));
        format!(
            "Invalid {} ({row}): {}{why}{cpu}",
            a.kind,
            a.exit.clone().unwrap_or_default()
        )
    }));
    l.push(format!(
        "Run sessions: {}",
        field(&r.sessions, u64::to_string)
    ));
    l.extend(r.stopped.as_ref().map(|s| format!("Stopped: {s}")));
    format!("{}\n{UNRECORDED}\n", list(l))
}

fn recipe_section(c: &Card) -> String {
    let (r, m) = (&c.recipe, &c.recipe.manifest);
    let mut l: Vec<String> = vec![];
    l.extend(r.requires.iter().map(|q| format!("Requires: {q}")));
    l.push(format!(
        "Isolation levels held: {}",
        word(&r.isolation_levels)
    ));
    l.push(format!(
        "Box version: {}",
        field(&r.box_version, |v| v.clone())
    ));
    l.push(format!(
        "Box binary SHA-256: {}",
        field(&r.box_binary_sha256, |v| v.clone())
    ));
    l.push(format!(
        "Bundle DOI: {}",
        field(&r.bundle_doi, |v| v.clone())
    ));
    l.push(format!("Manifest roots: {}", names(&m.roots)));
    let lacks = if m.missing.is_empty() {
        "none".into()
    } else {
        names(&m.missing)
    };
    l.push(format!("Roots the folder lacks: {lacks}"));
    l.push(format!(
        "Manifest: {} files, tree SHA-256 {}",
        m.files, m.tree_sha256
    ));
    format!("Command:\n\n{}\n\n{}", fence(&r.command, ""), list(l))
}

fn fenced_field(f: &Field<String>, lang: &str) -> String {
    match f {
        Field::Recorded { value } => format!("\n\n{}", fence(value, lang)),
        _ => esc(&field(f, |v| v.clone())),
    }
}

fn citation_section(c: &Card) -> String {
    format!(
        "BibTeX: {}\n\nCITATION.cff: {}\n",
        fenced_field(&c.citation.bibtex, "bibtex"),
        fenced_field(&c.citation.cff, "yaml")
    )
}

fn table_or_none(t: &Table) -> String {
    if t.rows.is_empty() {
        "None.\n".into()
    } else {
        md_table(t)
    }
}

/// The card as the Markdown of a README section, in the order of design 2.1.
pub fn markdown(c: &Card) -> String {
    let label = list(
        (c.verdicts.iter())
            .map(|v| format!("{}: {}", v.hypothesis, v.label))
            .collect(),
    );
    let readings = format!(
        "Not yet read by the author. The slots are in {}.\n",
        esc(&c.outside.readings)
    );
    let sections = [
        ("Header", header_section(c)),
        ("Claim", claim_section(c)),
        ("Verdicts", verdict_section(c)),
        ("Claim label", label),
        ("Results", table_or_none(&results_table(c))),
        ("Sensitivity", sensitivity_section(c)),
        ("Figures", figures_section(c)),
        ("Setup", setup_section(c)),
        ("Deviations", table_or_none(&deviations_table(c))),
        ("Surprises", surprises_section(c)),
        ("Cost", cost_section(c)),
        ("Runs", runs_section(c)),
        ("Re-run recipe", recipe_section(c)),
        ("Citation", citation_section(c)),
        ("Author readings", readings),
    ];
    let body: Vec<String> = (sections.iter())
        .map(|(h, b)| format!("## {h}\n\n{b}"))
        .collect();
    format!("# Study card: {}\n\n{}", esc(&c.study), body.join("\n"))
}
