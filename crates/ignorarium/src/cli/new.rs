//! `new`: copy a template into `studies/<id>/spec.toml`.
use super::{CliError, Done, Run, out};
use crate::files::{WriteOnceError, write_once};
use crate::rules::{self, Check};
use std::path::Path;

/// Embedded, so the binary works outside the repository.
const TEMPLATES: &[(&str, &str)] = &[
    (
        "numeric-ablation",
        include_str!("../../../../templates/numeric-ablation.toml"),
    ),
    (
        "planted-spawn",
        include_str!("../../../../templates/planted-spawn.toml"),
    ),
];

/// The id format is the one rule E-FMT-001 checks on `study.id`.
fn id_is_valid(id: &str) -> Result<bool, CliError> {
    rules::builtin()
        .iter()
        .find_map(|r| match &r.check {
            Check::Format(re) if r.code == "E-FMT-001" && r.path == "study.id" => {
                Some(re.is_match(id))
            }
            _ => None,
        })
        .ok_or_else(|| CliError::usage("rule E-FMT-001 for study.id is missing"))
}

/// Sets `id` inside the `[study]` table and keeps its comment. The templates
/// are ours: a table header is a line that starts with `[`.
fn with_id(template: &str, id: &str) -> String {
    let mut in_study = false;
    template
        .replace("\r\n", "\n")
        .split_inclusive('\n')
        .map(|line| {
            if line.starts_with('[') {
                in_study = line.trim_end().split('#').next().map(str::trim_end) == Some("[study]");
            }
            let tail = line
                .strip_prefix("id = \"")
                .and_then(|r| r.split_once('"'))
                .map(|(_, tail)| tail);
            match tail {
                Some(tail) if in_study => format!("id = \"{id}\"{tail}"),
                _ => line.to_string(),
            }
        })
        .collect()
}

pub fn run(template: &str, id: &str) -> Run {
    let Some((_, text)) = TEMPLATES.iter().find(|(n, _)| *n == template) else {
        let names: Vec<&str> = TEMPLATES.iter().map(|(n, _)| *n).collect();
        return Err(CliError::usage(format!(
            "unknown template \"{template}\"; choose one of: {}",
            names.join(", ")
        )));
    };
    if !id_is_valid(id)? {
        return Err(CliError::usage(format!(
            "\"{id}\" is not a valid id; use ASCII lowercase letters and digits joined by single hyphens"
        )));
    }
    let io = |e: std::io::Error| CliError::usage(format!("cannot create the study folder: {e}"));
    let studies = Path::new("studies");
    std::fs::create_dir_all(studies).map_err(io)?;
    let dir = studies.join(id);
    match std::fs::create_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(CliError::refuse(format!(
                "{} already exists",
                dir.display()
            )));
        }
        Err(e) => return Err(io(e)),
    }
    let file = dir.join("spec.toml");
    write_once(&file, with_id(text, id).as_bytes(), 0o644).map_err(|e| match e {
        WriteOnceError::Io(e) => io(e),
        other => CliError::usage(format!("cannot write {}: {other}", file.display())),
    })?;
    out(&format!("created {}\n", file.display()))?;
    Ok(Done {
        code: 0,
        spec_hash: None,
        leaf: None,
    })
}

#[cfg(test)]
mod tests {
    use super::with_id;

    #[test]
    fn only_id_inside_the_study_table_changes() {
        let text = "id = \"top\"\n[other]\nid = \"keep\"\n[study]\nid = \"my-study\" # c\n[[x]]\nid = \"H1\"\n[study.sub]\nid = \"s\"\n";
        assert_eq!(
            with_id(text, "new-one"),
            "id = \"top\"\n[other]\nid = \"keep\"\n[study]\nid = \"new-one\" # c\n[[x]]\nid = \"H1\"\n[study.sub]\nid = \"s\"\n"
        );
    }
}
