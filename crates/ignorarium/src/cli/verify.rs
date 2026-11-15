//! `verify`: check the plan logs and, for one study, its record chain.
use super::{CliError, Ctx, Done, Run, out};
use crate::derived::{Register, SessionEnd, SessionStart};
use crate::planlog::store::PlanLog;
use crate::records::Record;
use crate::telemetry::Reading;
use crate::{keys, records};
use std::collections::BTreeSet;

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
            match records::read_all(&dir.join("records.jsonl"), &k.verifying_key()) {
                Ok(all) if all.is_empty() => {
                    failed.push(format!("study {id}: no register record"));
                }
                Ok(all) => {
                    out(&format!("study {id}: {} records\n", all.len()))?;
                    // The register is the first record of the log.
                    let registered =
                        (all.first().and_then(Register::of)).and_then(|r| r.box_identity);
                    out(&format!(
                        "registered box: {}\n",
                        box_text(registered.as_ref())
                    ))?;
                    let (n, open) = sessions(&all);
                    out(&format!("sessions: {n}, {open} without an end record\n"))?;
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

/// The session starts in `all`, and how many of them have no end record of the same session.
fn sessions(all: &[Record]) -> (usize, usize) {
    let ended: BTreeSet<String> = (all.iter().filter_map(SessionEnd::of))
        .map(|e| e.session)
        .collect();
    let starts: Vec<SessionStart> = all.iter().filter_map(SessionStart::of).collect();
    let open = (starts.iter())
        .filter(|s| !ended.contains(&s.session))
        .count();
    (starts.len(), open)
}

/// The box identity a register record holds, as text; `not recorded` for a record written
/// before the field existed.
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

    #[test]
    fn a_session_pairs_with_the_end_of_its_own_id() {
        let record = |kind: &str, body: serde_json::Value| Record {
            seq: 0,
            prev: String::new(),
            study: "s".into(),
            row: None,
            kind: kind.into(),
            body,
            ts: String::new(),
        };
        let start = |id: &str| {
            let body = serde_json::json!({
                "session": id, "verb": "run", "pid": 1, "box_version": "0",
                "rules_sha256": "sha256:ab", "binary_sha256": "sha256:cd",
                "started": "2026-11-15T00:00:00Z",
            });
            record(crate::derived::SESSION_START, body)
        };
        let end = |id: &str| {
            let body = serde_json::json!({ "session": id, "status": "done" });
            record(crate::derived::SESSION_END, body)
        };
        assert_eq!(sessions(&[]), (0, 0));
        assert_eq!(sessions(&[start("a"), end("a")]), (1, 0));
        assert_eq!(sessions(&[start("a"), start("b"), end("b")]), (2, 1));
        assert_eq!(sessions(&[start("a"), end("b")]), (1, 1));
    }
}
