//! `import`: add files to the content store and sign one record per file.
use super::history::regular_file;
use super::{CliError, Ctx, Done, Run, out};
use crate::keys;
use crate::records::{self, RecordDraft};
use crate::scan;
use crate::store::Store;
use serde_json::json;
use std::path::PathBuf;

pub fn run(ctx: &Ctx, source: &str, files: &[PathBuf]) -> Run {
    let home = ctx.home()?;
    if source.trim().is_empty() {
        return Err(CliError::usage("--source needs text"));
    }
    // Text that enters a signed record cannot be edited later. Errors name the kind only.
    let flagged = |text: &str, what: &str| match scan::scan_str(text, scan::builtin()).first() {
        Some(kind) => Err(CliError::refuse(format!(
            "{what} looks like a secret or personal data ({kind}); nothing was imported"
        ))),
        None => Ok(()),
    };
    flagged(source, "--source")?;
    // Every file is checked before the store changes, so a bad file adds nothing.
    for f in files {
        regular_file(f)?;
        flagged(&record_name(f), "a file name")?;
    }
    let key = keys::load_or_create(home).map_err(|e| CliError::usage(format!("box key: {e}")))?;
    let store = Store::open(&home.root).map_err(|e| CliError::usage(format!("store: {e}")))?;
    let log = store.root.join("imports.jsonl");
    for f in files {
        // The record keeps the name; the path as given is printed, never signed.
        let shown = f.display().to_string();
        let (digest, meta) = store
            .import(f)
            .map_err(|e| CliError::usage(format!("{shown}: {e}")))?;
        records::append(
            &log,
            &key,
            RecordDraft {
                study: "store".into(),
                row: None,
                kind: "import".into(),
                body: json!({
                    "digest": digest, "size": meta.size,
                    "normalized_sha256": meta.normalized_sha256,
                    "source": source, "file": record_name(f),
                }),
            },
        )
        .map_err(|e| CliError::usage(format!("imports.jsonl: {e}")))?;
        out(&format!("{digest}  {shown}\n"))?;
    }
    Ok(Done::exit(0))
}

fn record_name(f: &std::path::Path) -> String {
    f.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}
