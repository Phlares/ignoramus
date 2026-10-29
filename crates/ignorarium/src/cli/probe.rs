//! `probe`: measure this machine and write `machine.toml`.
use super::{CliError, Ctx, Done, Run, out};
use crate::{backend, machine};
use serde_json::json;
use std::path::Path;

pub fn run(ctx: &Ctx) -> Run {
    let home = ctx.home()?;
    let m = machine::probe(&home.root, &backend::registered())
        .map_err(|e| CliError::refuse(e.to_string()))?;
    machine::write(&home.root, &m).map_err(|e| CliError::usage(e.to_string()))?;
    let kvm = Path::new("/dev/kvm").exists();
    let note = if kvm {
        "present (level 2 needs a backend this build lacks)"
    } else {
        "absent (level 2 is not possible here)"
    };
    out(&format!(
        "machine: {}\ncpus: {}\nmemory: {} GiB\nmax level: {}\n/dev/kvm: {note}\n",
        m.name, m.cpus, m.mem_gib, m.max_level
    ))?;
    Ok(Done {
        attrs: Some(json!({ "max_level": m.max_level, "kvm": kvm })),
        ..Done::exit(0)
    })
}
