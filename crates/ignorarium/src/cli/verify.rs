//! `verify`: check the plan logs and, for one study, its record chain.
use super::{CliError, Ctx, Done, Run, out};
use crate::planlog::store::PlanLog;
use crate::telemetry::Reading;
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
                Ok(0) => failed.push(format!("study {id}: no register record")),
                Ok(n) => {
                    out(&format!("study {id}: {n} records\n"))?;
                    let shown = registered_box(&dir, &k.verifying_key());
                    out(&format!("registered box: {shown}\n"))?;
                }
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

/// The box identity the study's register record holds, as text; `not recorded` for a record
/// written before the field existed. The register is the first record of the log.
fn registered_box(dir: &std::path::Path, key: &ed25519_dalek::VerifyingKey) -> String {
    // A read error prints `not recorded`: the log verified one line earlier.
    let register = records::first(&dir.join("records.jsonl"), key)
        .ok()
        .flatten()
        .and_then(|r| crate::derived::Register::of(&r))
        .and_then(|r| r.box_identity);
    box_text(register.as_ref())
}

fn box_text(b: Option<&crate::identity::BoxIdentity>) -> String {
    let Some(b) = b else {
        return "not recorded".into();
    };
    let binary = match &b.binary_sha256 {
        Reading::Read(s) => s.clone(),
        Reading::Unavailable(why) => format!("unavailable: {why}"),
    };
    format!("rules {}, binary {binary}", b.rules_sha256)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::Reading;

    #[test]
    fn an_unavailable_binary_prints_its_reason() {
        let b = crate::identity::BoxIdentity {
            rules_sha256: "sha256:ab".into(),
            binary_sha256: Reading::Unavailable("no exe".into()),
        };
        assert_eq!(
            box_text(Some(&b)),
            "rules sha256:ab, binary unavailable: no exe"
        );
        assert_eq!(box_text(None), "not recorded");
    }
}
