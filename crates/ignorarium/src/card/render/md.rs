//! The card as Markdown.
use super::svg::figure_file;
use super::{
    Table, alt_text, claim_text, deviations_table, e_text, field, flat, group, pct, results_table,
    sensitivity_table, sig4, threshold_text, word,
};
use crate::card::Card;
use crate::card::readings::NOT_READ;
use crate::card::sections::{Field, UNRECORDED};
use crate::spec::model::StatisticSpec;

fn md_table(t: &Table) -> String {
    let line = |cells: Vec<String>| format!("| {} |\n", cells.join(" | "));
    let mut s = line(t.head.iter().map(|h| h.to_string()).collect());
    s += &line(t.head.iter().map(|_| "---".to_string()).collect());
    for r in &t.rows {
        s += &line(r.iter().map(|c| esc(c)).collect());
    }
    s
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
                    format!(
                        "; pass rule: e-value at least {} in {groups}",
                        threshold_text(e)
                    )
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
            format!("- ![{alt}]({})\n", figure_file(&l.hypothesis))
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
    l.extend(c.outside.files.iter().map(|f| {
        let why = f.note.as_ref().map(|n| format!(" ({n})"));
        format!(
            "File {}: {}{}",
            f.path,
            sha(&f.sha256),
            why.unwrap_or_default()
        )
    }));
    l.extend(c.outside_source.iter().map(|o| {
        format!(
            "Outside files pinned at commit {}, folder \"{}\"",
            o.commit, o.path
        )
    }));
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
            threshold_text(t.threshold),
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
        format!(
            "Host CPU ticks summed over rows, seconds (rows that ran at the same time each count the same host ticks): {}",
            word(&t.cpu_ticks.value)
        ),
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
    let (first, rest) = NOT_READ.split_at(1);
    let readings = format!(
        "{}{rest}. The slots are in {}.\n",
        first.to_uppercase(),
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
