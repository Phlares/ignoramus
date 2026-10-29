//! `probe`: measure this machine and write `machine.toml`.
use super::{CliError, Ctx, Done, Run, out};
use crate::backend::{self, Probe};
use crate::machine;
use serde_json::json;
use std::path::Path;

pub fn run(ctx: &Ctx) -> Run {
    let home = ctx.home()?;
    let backends = backend::registered();
    let probes = machine::probe_backends(&backends);
    let m = machine::measure(&home.root, &probes).map_err(|e| CliError::refuse(e.to_string()))?;
    machine::write(&home.root, &m).map_err(|e| CliError::usage(e.to_string()))?;
    let kvm = Path::new("/dev/kvm").exists();
    let note = if kvm {
        "present (level 2 needs a backend this build lacks)"
    } else {
        "absent (level 2 is not possible here)"
    };
    let mut report = String::new();
    for (b, (level, probe)) in backends.iter().zip(&probes) {
        let answer = match probe {
            Probe::Available => "available".to_string(),
            Probe::Unavailable(why) => format!("unavailable: {why}"),
        };
        report.push_str(&format!(
            "level {level} ({}): {answer}
",
            b.launcher()
        ));
    }
    out(&format!(
        "machine: {}
cpus: {}
memory: {} GiB
{report}max level: {}
/dev/kvm: {note}
",
        m.name, m.cpus, m.mem_gib, m.max_level
    ))?;
    Ok(Done {
        attrs: Some(json!({ "max_level": m.max_level, "kvm": kvm })),
        ..Done::exit(0)
    })
}
