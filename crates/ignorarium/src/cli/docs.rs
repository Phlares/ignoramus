//! `dev gen-rule-docs`: one Markdown page per rule code, from the built-in rules.
use super::{CliError, Done, Run, out};
use crate::rules::{self, Check, Rule};
use std::collections::BTreeMap;
use std::path::Path;

fn page(code: &str, rules: &[&Rule]) -> String {
    let first = rules[0];
    let mut s = format!(
        "# {code}\n\nSeverity: {}\n\n## Problem\n\n{}\n\n## Fix\n\n{}\n\n## Why\n\n{}\n\n",
        first.severity.as_str(),
        first.problem,
        first.instruction,
        first.why
    );
    if !first.sources.is_empty() {
        s.push_str("## Sources\n\n");
        for url in &first.sources {
            s.push_str(&format!("- <{url}>\n"));
        }
        s.push('\n');
    }
    s.push_str("## Applies to\n\n");
    for r in rules {
        if matches!(r.check, Check::Documented) {
            s.push_str(
                "- the command line, not a spec field
",
            );
            continue;
        }
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

/// Writes one page per code. It removes nothing: the up-to-date test catches stale pages.
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
    out(&format!("wrote {} rule pages\n", by_code.len()))?;
    Ok(Done {
        code: 0,
        spec_hash: None,
        leaf: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Severity;

    fn rule(sources: Vec<String>) -> Rule {
        Rule {
            code: "E-X-001".into(),
            severity: Severity::Error,
            path: "a.b".into(),
            check: Check::Required,
            problem: "P.".into(),
            instruction: "I.".into(),
            why: "W.".into(),
            sources,
            when: None,
        }
    }

    #[test]
    fn a_rule_without_sources_has_no_sources_heading() {
        let none = page("E-X-001", &[&rule(vec![])]);
        assert!(!none.contains("## Sources"), "{none}");
        assert!(none.contains("## Why\n\nW.\n\n## Applies to"), "{none}");
        let some = page("E-X-001", &[&rule(vec!["https://a.test/".into()])]);
        assert!(some.contains("## Sources\n\n- <https://a.test/>\n\n## Applies to"));
    }
}
