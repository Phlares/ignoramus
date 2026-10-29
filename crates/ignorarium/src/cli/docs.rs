//! `dev gen-rule-docs`: one Markdown page per rule code, from the built-in rules.
use super::{CliError, Done, Run, out};
use crate::rules::{self, Rule};
use std::collections::BTreeMap;
use std::path::Path;

fn page(code: &str, rules: &[&Rule]) -> String {
    let first = rules[0];
    let mut s = format!(
        "# {code}\n\nSeverity: {}\n\n## Problem\n\n{}\n\n## Fix\n\n{}\n\n## Why\n\n{}\n\n## Sources\n\n",
        first.severity.as_str(),
        first.problem,
        first.instruction,
        first.why
    );
    for url in &first.sources {
        s.push_str(&format!("- <{url}>\n"));
    }
    s.push_str("\n## Applies to\n\n");
    for r in rules {
        s.push_str(&format!("- `{}` ({})", r.path, r.check.name()));
        if let Some(w) = &r.when {
            let gates: Vec<String> = [("mode", &w.mode), ("label", &w.label)]
                .into_iter()
                .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k} = {v}")))
                .collect();
            s.push_str(&format!(", when {}", gates.join(" and ")));
        }
        s.push('\n');
    }
    s
}

pub fn run(dir: &Path) -> Run {
    let mut by_code: BTreeMap<&str, Vec<&Rule>> = BTreeMap::new();
    for r in rules::builtin() {
        by_code.entry(&r.code).or_default().push(r);
    }
    let io = |e: std::io::Error| CliError::usage(format!("{}: {e}", dir.display()));
    std::fs::create_dir_all(dir).map_err(io)?;
    for (code, rs) in &by_code {
        std::fs::write(dir.join(format!("{code}.md")), page(code, rs)).map_err(io)?;
    }
    out(&format!("wrote {} rule pages\n", by_code.len()));
    Ok(Done {
        code: 0,
        spec_hash: None,
    })
}
