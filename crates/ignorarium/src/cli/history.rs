//! `import-history`: record earlier data files as one `legacy` plan log leaf.
use super::{CliError, Ctx, Done, Run, out};
use crate::planlog::store::{Leaf, PlanLog};
use crate::prior::file_hashes;
use crate::{keys, time};
use std::path::PathBuf;

pub fn run(ctx: &Ctx, phase: &str, files: &[PathBuf]) -> Run {
    let home = ctx.home()?;
    if phase.is_empty() {
        return Err(CliError::usage("--phase needs a name"));
    }
    // Every file is read before the log changes, so a bad file adds nothing.
    let mut data_hashes = Vec::new();
    for f in files {
        let shown = f.display();
        let regular = std::fs::symlink_metadata(f)
            .map_err(|e| CliError::usage(format!("{shown}: {e}")))?
            .is_file();
        if !regular {
            return Err(CliError::usage(format!("{shown}: not a regular file")));
        }
        let bytes = std::fs::read(f).map_err(|e| CliError::usage(format!("{shown}: {e}")))?;
        data_hashes.push(file_hashes(&bytes));
    }
    let key = keys::load_or_create(home).map_err(|e| CliError::usage(format!("box key: {e}")))?;
    let (index, _) = PlanLog::open(&home.root.join("planlog"))
        .and_then(|log| {
            log.append(
                Leaf {
                    kind: "legacy".into(),
                    commitment: None,
                    family: None,
                    mode: None,
                    data_hashes,
                    label: phase.into(),
                    ts: time::now_rfc3339(),
                },
                &key,
            )
        })
        .map_err(|e| CliError::usage(format!("planlog: {e}")))?;
    out(&format!("leaf {index}\n"))?;
    Ok(Done {
        leaf: Some(index),
        ..Done::exit(0)
    })
}
