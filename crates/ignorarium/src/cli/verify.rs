//! `verify`: check the plan logs and, for one study, its record chain.
use super::{CliError, Ctx, Done, Run, out};
use crate::planlog::store::PlanLog;
use crate::{keys, records};

pub fn run(ctx: &Ctx, study: Option<&str>) -> Run {
    let home = ctx.home()?;
    let key = keys::load_or_create(home)
        .map_err(|e| CliError::usage(format!("box key: {e}")))?
        .verifying_key();
    let mut failed = Vec::new();
    for name in ["planlog", "testlog"] {
        let result = PlanLog::open(&home.root.join(name)).and_then(|l| l.verify(&key));
        match result {
            Ok(v) => {
                out(&format!(
                    "{name}: {} signed, {} leaves\n",
                    v.signed, v.leaves
                ))?;
                if v.leaves > v.signed {
                    out(&format!(
                        "warning: {name} has {} unsigned leaves; the next append signs them\n",
                        v.leaves - v.signed
                    ))?;
                }
            }
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    if let Some(id) = study {
        let known = !id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        let dir = home.study_dir(id);
        if !known || !dir.is_dir() {
            return Err(CliError::usage(format!("no study named {id:?} in the box")));
        }
        match records::verify(&dir.join("records.jsonl"), &key) {
            Ok(n) => out(&format!("study {id}: {n} records\n"))?,
            Err(e) => failed.push(format!("study {id}: {e}")),
        }
    }
    if failed.is_empty() {
        Ok(Done {
            code: 0,
            spec_hash: None,
            leaf: None,
        })
    } else {
        Err(CliError::refuse(format!(
            "verify failed: {}",
            failed.join("; ")
        )))
    }
}
