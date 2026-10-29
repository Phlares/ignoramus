//! `check`: load a spec, run every rule, and report. Shared with `review`.
use super::{CliError, Ctx, Done, Run, out};
use crate::diag::{Diagnostic, Severity, render_human};
use crate::estimate::{Estimate, defaults_json, estimate, units};
use crate::planlog::store::PlanLog;
use crate::prior::{PriorError, PriorUse, file_hashes};
use crate::rules::{self, CheckCtx, input_bytes};
use crate::spec::{self, LoadError, model::Spec};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Everything a check finds about one spec file.
pub struct Assessed {
    /// Hash of the author's file, never of the defaulted value.
    pub hash: String,
    /// The spec with defaults filled.
    pub value: Value,
    pub filled: Vec<(String, Value)>,
    pub diags: Vec<Diagnostic>,
    /// Present only when the plan has no errors.
    pub estimate: Option<Estimate>,
    /// `(path, (exact, normalized) SHA-256)` of each input, read once here; `None`
    /// when the file is absent or its path is unsafe. `register` stores these.
    pub input_hashes: Vec<(String, Option<(String, String)>)>,
}

impl Assessed {
    pub fn has_errors(&self) -> bool {
        self.diags.iter().any(|d| d.severity == Severity::Error)
    }

    pub fn exit_code(&self) -> i32 {
        i32::from(self.has_errors())
    }
}

/// A plan log that cannot be opened: every lookup fails, so the rules refuse.
struct Unreadable(String);

impl PriorUse for Unreadable {
    fn used(&self, _: &str, _: &str) -> Result<Option<String>, PriorError> {
        Err(PriorError(self.0.clone()))
    }
}

/// Hash every input the spec lists. This is the one read `register` relies on.
pub fn input_hashes(study_dir: &Path, spec: &Value) -> Vec<(String, Option<(String, String)>)> {
    let paths = spec["inputs"].as_array().map_or(&[][..], Vec::as_slice);
    paths
        .iter()
        .filter_map(|i| i["path"].as_str())
        .map(|p| {
            (
                p.to_string(),
                input_bytes(study_dir, p).map(|b| file_hashes(&b)),
            )
        })
        .collect()
}

/// The folder an input path is relative to: the spec's own folder.
pub fn study_dir(spec_path: &Path) -> PathBuf {
    match spec_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

pub fn assess(ctx: &Ctx, spec_path: &Path) -> Result<Assessed, CliError> {
    let home = ctx.home()?;
    let shown = spec_path.display();
    let raw = spec::load(spec_path).map_err(|e| match e {
        LoadError::Io(_) => CliError::usage(format!("{shown}: {e}")),
        _ => CliError::refuse(format!("{shown}: {e}")),
    })?;
    let machine = crate::machine::load(&home.root).map_err(|e| CliError::usage(e.to_string()))?;
    let prior: Box<dyn PriorUse> = match PlanLog::open(&home.root.join("planlog")) {
        Ok(log) => Box::new(log),
        Err(e) => Box::new(Unreadable(e.to_string())),
    };
    let cc = CheckCtx {
        study_dir: study_dir(spec_path),
        prior,
        machine_max_level: machine.max_level,
        spec_text: raw.text,
    };
    let (value, filled, diags) = rules::check_all(&raw.value, rules::builtin(), &cc);
    let mut a = Assessed {
        hash: raw.hash,
        input_hashes: input_hashes(&cc.study_dir, &value),
        value,
        filled,
        diags,
        estimate: None,
    };
    if !a.has_errors() {
        let s = Spec::from_value(&a.value)
            .map_err(|e| CliError::usage(format!("{shown}: spec does not fit the schema: {e}")))?;
        a.estimate = Some(estimate(&s, &machine, units(&s)));
    }
    Ok(a)
}

/// `<id>: <n> rows`, one per condition.
pub fn condition_lines(e: &Estimate) -> Vec<String> {
    e.rows_per_condition
        .iter()
        .map(|(id, n)| format!("{id}: {n} rows"))
        .collect()
}

pub fn figure_lines(e: &Estimate) -> Vec<String> {
    vec![
        format!("rows: {}", e.rows),
        format!("cpu hours: {:.2}", e.cpu_hours),
        format!("wall hours: {:.2}", e.wall_hours),
        format!("money: ${:.2}", e.money_usd),
        format!("long job: {}", if e.is_long_job { "yes" } else { "no" }),
    ]
}

pub fn default_lines(a: &Assessed) -> Vec<String> {
    a.filled
        .iter()
        .map(|(path, v)| format!("{path} = {v}"))
        .collect()
}

fn indented(lines: Vec<String>) -> String {
    lines.iter().map(|l| format!("  {l}\n")).collect()
}

fn render(a: &Assessed) -> String {
    let mut s = String::new();
    if !a.diags.is_empty() {
        s.push_str(&render_human(&a.diags));
        s.push('\n');
    }
    if let Some(e) = &a.estimate {
        s.push_str(&format!("conditions:\n{}\n", indented(condition_lines(e))));
        s.push_str(&format!("estimate:\n{}\n", indented(figure_lines(e))));
        s.push_str(&format!("label: {}\n\n", e.label_earnable));
        s.push_str(&format!("defaults:\n{}\n", indented(default_lines(a))));
    }
    s.push_str(&format!("spec: {}\n", a.hash));
    s
}

fn render_json(a: &Assessed) -> Result<String, CliError> {
    let estimate = a.estimate.as_ref().map(|e| {
        json!({
            "rows": e.rows,
            "rows_per_condition": e.rows_per_condition.iter()
                .map(|(id, rows)| json!({ "id": id, "rows": rows }))
                .collect::<Vec<_>>(),
            "cpu_hours": e.cpu_hours,
            "wall_hours": e.wall_hours,
            "money_usd": e.money_usd,
            "label": e.label_earnable,
            "long_job": e.is_long_job,
        })
    });
    let v = json!({
        "diagnostics": a.diags,
        "estimate": estimate,
        "defaults": defaults_json(&a.filled),
        "hash": a.hash,
    });
    serde_json::to_string_pretty(&v).map_err(|e| CliError::usage(e.to_string()))
}

pub fn run(ctx: &Ctx, spec_path: &Path, json: bool) -> Run {
    let a = match assess(ctx, spec_path) {
        Err(e) if json => {
            let v = json!({ "diagnostics": [], "error": e.msg, "hash": null });
            let text =
                serde_json::to_string_pretty(&v).map_err(|e| CliError::usage(e.to_string()))?;
            out(&format!("{text}\n"))?;
            return Ok(Done::exit(e.code));
        }
        other => other?,
    };
    let text = if json {
        render_json(&a)? + "\n"
    } else {
        render(&a)
    };
    out(&text)?;
    let code = a.exit_code();
    Ok(Done {
        spec_hash: Some(a.hash),
        ..Done::exit(code)
    })
}
