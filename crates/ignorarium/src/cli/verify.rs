//! `verify`: check the plan logs and, for one study, its record chain.
use super::{CliError, Ctx, Done, Run, out};
use crate::derived::{Register, SESSION_END, SESSION_START, SessionEnd, SessionStart};
use crate::identity::{binary_text, same_binary};
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
                    let starts: Vec<SessionStart> =
                        all.iter().filter_map(SessionStart::of).collect();
                    let (open, unreadable) = sessions(&all, &starts);
                    let unreadable = match unreadable {
                        0 => String::new(),
                        m => format!(", {m} unreadable"),
                    };
                    out(&format!(
                        "sessions: {}, {open} without an end record{unreadable}\n",
                        starts.len()
                    ))?;
                    let was = registered.as_ref().map(|b| &b.binary_sha256);
                    for (k, declared) in binary_changes(was, &starts) {
                        let how = match &declared {
                            Some(why) => format!("declared ({why})"),
                            None => "undeclared".into(),
                        };
                        out(&format!("binary changed at session {k}: {how}\n"))?;
                        if declared.is_none() {
                            failed.push(format!(
                                "study {id}: undeclared binary change at session {k}"
                            ));
                        }
                    }
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

/// Of `starts`, every session start that parses in `all`: how many have no end record of the
/// same session, and how many session records in `all` have a body that does not parse.
fn sessions(all: &[Record], starts: &[SessionStart]) -> (usize, usize) {
    let ends: Vec<SessionEnd> = all.iter().filter_map(SessionEnd::of).collect();
    let ended: BTreeSet<&str> = ends.iter().map(|e| &e.session[..]).collect();
    let open = (starts.iter())
        .filter(|s| !ended.contains(&s.session[..]))
        .count();
    let kinds = (all.iter())
        .filter(|r| r.kind == SESSION_START || r.kind == SESSION_END)
        .count();
    (open, kinds.saturating_sub(starts.len() + ends.len()))
}

/// Each binary change between consecutive session starts, or between the `registered` binary
/// and the first start: the start's 1-based number, and its reason when its `changed_from` is
/// the binary before it (`None`: undeclared). With no registered binary, the first start has
/// nothing to compare. An unreadable binary always differs, as at the gate; a change from one
/// is declared when `changed_from` holds that same reading, as the gate writes it.
fn binary_changes(
    registered: Option<&Reading<String>>,
    starts: &[SessionStart],
) -> Vec<(usize, Option<String>)> {
    let before = std::iter::once(registered).chain(starts.iter().map(|s| Some(&s.binary_sha256)));
    (before.zip(starts).enumerate())
        .filter_map(|(i, (was, s))| {
            let was = was.filter(|w| !same_binary(w, &s.binary_sha256))?;
            let declared = s.changed_from.as_ref() == Some(was);
            Some((
                i + 1,
                declared.then(|| s.reason.clone().unwrap_or_default()),
            ))
        })
        .collect()
}

/// The box identity a register record holds, as text; `not recorded` for a record written
/// before the field existed.
fn box_text(b: Option<&crate::identity::BoxIdentity>) -> String {
    let Some(b) = b else {
        return "not recorded".into();
    };
    let binary = binary_text(&b.binary_sha256);
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
        // The starts as `run` parses them, then what `sessions` counts.
        let sessions = |all: &[Record]| {
            let starts: Vec<SessionStart> = all.iter().filter_map(SessionStart::of).collect();
            let (open, unreadable) = sessions(all, &starts);
            (starts.len(), open, unreadable)
        };
        assert_eq!(sessions(&[]), (0, 0, 0));
        assert_eq!(sessions(&[start("a"), end("a")]), (1, 0, 0));
        assert_eq!(sessions(&[start("a"), start("b"), end("b")]), (2, 1, 0));
        assert_eq!(sessions(&[start("a"), end("b")]), (1, 1, 0));
        // A session record whose body does not parse counts as unreadable, not as a session.
        let bad = |kind: &str| record(kind, serde_json::json!({ "session": 1 }));
        let all = [
            start("a"),
            bad(crate::derived::SESSION_START),
            bad(crate::derived::SESSION_END),
        ];
        assert_eq!(sessions(&all), (1, 1, 2));
    }

    #[test]
    fn each_binary_change_is_numbered_by_its_session() {
        let read = |c: &str| Reading::Read(format!("sha256:{c}"));
        let start =
            |binary: Reading<String>, declared: Option<(Reading<String>, &str)>| SessionStart {
                session: "a".into(),
                verb: "run".into(),
                pid: 1,
                box_version: "0".into(),
                rules_sha256: "sha256:ab".into(),
                binary_sha256: binary,
                started: "2026-11-15T00:00:00Z".into(),
                reason: declared.as_ref().map(|(_, why)| why.to_string()),
                changed_from: declared.map(|(from, _)| from),
            };
        let (a, b) = (read("aa"), read("bb"));
        // A register without `box` gives the first session nothing to compare.
        assert_eq!(binary_changes(None, &[start(b.clone(), None)]), []);
        assert_eq!(binary_changes(Some(&a), &[start(a.clone(), None)]), []);
        assert_eq!(
            binary_changes(Some(&a), &[start(b.clone(), None)]),
            [(1, None)]
        );
        let starts = [
            start(a.clone(), None),
            start(b.clone(), Some((a.clone(), "fix"))),
            start(b.clone(), None),
        ];
        assert_eq!(
            binary_changes(Some(&a), &starts),
            [(2, Some("fix".to_string()))]
        );
        // A change that names another binary than the one before it is undeclared.
        let starts = [start(b.clone(), Some((read("cc"), "fix")))];
        assert_eq!(binary_changes(Some(&a), &starts), [(1, None)]);
        // An unreadable binary is never the same as another reading, and a change from one is
        // declared when it names that reading, as the gate writes it.
        let blind = Reading::Unavailable("no exe".into());
        let starts = [start(blind.clone(), None), start(blind.clone(), None)];
        assert_eq!(binary_changes(Some(&a), &starts), [(1, None), (2, None)]);
        let starts = [start(b.clone(), Some((blind.clone(), "fix")))];
        assert_eq!(
            binary_changes(Some(&blind), &starts),
            [(1, Some("fix".to_string()))]
        );
    }
}
