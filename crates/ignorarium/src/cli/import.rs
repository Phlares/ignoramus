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
    // The text enters a signed record that cannot be edited. The error names the kind only.
    if let Some(kind) = scan::scan_str(source, scan::builtin()).first() {
        return Err(CliError::refuse(format!(
            "--source looks like a secret or personal data ({kind}); nothing was imported"
        )));
    }
    // Every file is checked before the store changes, so a bad file adds nothing.
    for f in files {
        regular_file(f)?;
    }
    let key = keys::load_or_create(home).map_err(|e| CliError::usage(format!("box key: {e}")))?;
    let store = Store::open(&home.root).map_err(|e| CliError::usage(format!("store: {e}")))?;
    let log = store.root.join("imports.jsonl");
    for f in files {
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
                    "source": source, "file": shown,
                }),
            },
        )
        .map_err(|e| CliError::usage(format!("imports.jsonl: {e}")))?;
        out(&format!("{digest}  {shown}\n"))?;
    }
    Ok(Done::exit(0))
}
