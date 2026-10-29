//! `verify`: check the plan logs and, for one study, its record chain.
use super::{CliError, Ctx, Done, Run, out};
use crate::planlog::store::PlanLog;
use crate::{keys, records};

/// True when the log holds no leaves.
fn empty(home: &crate::home::Home, name: &str) -> bool {
    std::fs::metadata(home.root.join(name).join("leaves.jsonl"))
        .ok()
        .is_none_or(|m| m.len() == 0)
}

pub fn run(ctx: &Ctx, study: Option<&str>) -> Run {
    let home = ctx.home()?;
    let key = keys::load(home).map_err(|e| CliError::usage(format!("box key: {e}")))?;
    let mut failed = Vec::new();
    match &key {
        None if study.is_none() && ["planlog", "testlog"].iter().all(|n| empty(home, n)) => {
            out("nothing to verify: no box key and no log entries\n")?;
            return Ok(Done::exit(0));
        }
        None => failed.push("no box key".to_string()),
        Some(k) => {
            for name in ["planlog", "testlog"] {
                let result =
                    PlanLog::open(&home.root.join(name)).and_then(|l| l.verify(&k.verifying_key()));
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
        }
    }
    if let Some(id) = study {
        let known = !id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        let dir = home.study_dir(id);
        if !known || !dir.is_dir() {
            failed.push(format!("no study named {id:?} in the box"));
        } else if let Some(k) = &key {
            match records::verify(&dir.join("records.jsonl"), &k.verifying_key()) {
                Ok(n) => out(&format!("study {id}: {n} records\n"))?,
                Err(e) => failed.push(format!("study {id}: {e}")),
            }
        }
    }
    if failed.is_empty() {
        Ok(Done::exit(0))
    } else {
        Err(CliError::refuse(format!(
            "verify failed: {}",
            failed.join("; ")
        )))
    }
}
