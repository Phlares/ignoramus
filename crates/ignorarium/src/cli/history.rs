//! `import-history`: record earlier data files as one `legacy` plan log leaf.
use super::{CliError, Ctx, Done, Run, out};
use crate::keys;
use crate::planlog::store::PlanLog;
use crate::prior::file_hashes;
use crate::runner::OWN_PREFIX;
use std::path::{Path, PathBuf};

/// Refuse a path that is missing, a link, or not a regular file.
pub(super) fn regular_file(f: &Path) -> Result<(), CliError> {
    let shown = f.display();
    let regular = std::fs::symlink_metadata(f)
        .map_err(|e| CliError::usage(format!("{shown}: {e}")))?
        .is_file();
    if regular {
        Ok(())
    } else {
        Err(CliError::usage(format!("{shown}: not a regular file")))
    }
}

pub fn run(ctx: &Ctx, phase: &str, files: &[PathBuf]) -> Run {
    let home = ctx.home()?;
    if phase.is_empty() {
        return Err(CliError::usage("--phase needs a name"));
    }
    if phase.starts_with(OWN_PREFIX) {
        return Err(CliError::usage(format!(
            "--phase cannot start with {OWN_PREFIX}, which marks the data a study recorded itself"
        )));
    }
    // Every file is read before the log changes, so a bad file adds nothing.
    let mut data_hashes = Vec::new();
    for f in files {
        regular_file(f)?;
        let bytes =
            std::fs::read(f).map_err(|e| CliError::usage(format!("{}: {e}", f.display())))?;
        data_hashes.push(file_hashes(&bytes));
    }
    let key = keys::load_or_create(home).map_err(|e| CliError::usage(format!("box key: {e}")))?;
    let (index, _) = PlanLog::open(&home.root.join("planlog"))
        .and_then(|log| log.append_legacy(phase, data_hashes, &key))
        .map_err(|e| CliError::usage(format!("planlog: {e}")))?;
    out(&format!("leaf {index}\n"))?;
    Ok(Done {
        leaf: Some(index),
        ..Done::exit(0)
    })
}
