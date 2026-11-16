//! `verify`: check the plan logs and, for one study, its record chain; or check a copy of a
//! study's log with a public key alone.
use super::{CliError, Ctx, Done, Run, key, out};
use crate::derived::{DONE, DoneRecord, OPENED, Register, STAMP, SessionStart, sessions};
use crate::identity::same_binary;
use crate::planlog::store::PlanLog;
use crate::telemetry::Reading;
use crate::{keys, records};
use ed25519_dalek::VerifyingKey;
use std::path::Path;

/// True when the log holds no leaves.
fn empty(home: &crate::home::Home, name: &str) -> bool {
    std::fs::metadata(home.root.join(name).join("leaves.jsonl"))
        .ok()
        .is_none_or(|m| m.len() == 0)
}

/// Check the log at `records` with the public key in `public_key`. Opens no box home.
pub fn run_public(public_key: &Path, records: &Path) -> Run {
    let key = key::read_public(public_key)?;
    if !records.is_file() {
        return Err(CliError::usage("cannot read the records file"));
    }
    let mut failed = Vec::new();
    finish(check_study(None, records, &key), &mut failed)?;
    fail_or_pass(failed)
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
            let check = check_study(Some(id), &dir.join("records.jsonl"), &k.verifying_key());
            finish(check, &mut failed)?;
        }
    }
    fail_or_pass(failed)
}

/// Print what a study check found; its failures join `failed`.
fn finish(check: StudyCheck, failed: &mut Vec<String>) -> Result<(), CliError> {
    for line in &check.lines {
        out(&format!("{line}\n"))?;
    }
    failed.extend(check.failed);
    Ok(())
}

/// Exit 0 when nothing failed; otherwise a refusal that lists every failure.
fn fail_or_pass(failed: Vec<String>) -> Run {
    if failed.is_empty() {
        Ok(Done::exit(0))
    } else {
        Err(CliError::refuse(format!(
            "verify failed: {}",
            failed.join("; ")
        )))
    }
}

/// What one study's log shows: the lines to print and the failures found.
pub(crate) struct StudyCheck {
    pub lines: Vec<String>,
    pub failed: Vec<String>,
}

/// Check the chain of the log at `log` with `key`, and what it holds. `id` names the study in
/// the lines; without it the study is the one the first record names.
pub(crate) fn check_study(id: Option<&str>, log: &Path, key: &VerifyingKey) -> StudyCheck {
    let mut c = StudyCheck {
        lines: Vec::new(),
        failed: Vec::new(),
    };
    let all = match records::read_all(log, key) {
        Ok(all) => all,
        Err(e) => {
            c.failed
                .push(format!("{}{e}", id.map_or(String::new(), study_prefix)));
            return c;
        }
    };
    let Some(first) = all.first() else {
        let name = id.map_or(String::new(), study_prefix);
        c.failed.push(format!("{name}no register record"));
        return c;
    };
    let id = id.unwrap_or(&first.study);
    c.lines.push(format!("study {id}: {} records", all.len()));
    // The register is the first record of the log.
    let registered = (all.first().and_then(Register::of)).and_then(|r| r.box_identity);
    c.lines
        .push(format!("registered box: {}", box_text(registered.as_ref())));
    let starts: Vec<SessionStart> = all.iter().filter_map(SessionStart::of).collect();
    let counts = sessions(&all);
    let unreadable = match counts.unreadable {
        0 => String::new(),
        m => format!(", {m} unreadable"),
    };
    c.lines.push(format!(
        "sessions: {}, {} without an end record{unreadable}",
        counts.started, counts.without_end
    ));
    let was = registered.as_ref().map(|b| &b.binary_sha256);
    for (k, declared) in binary_changes(was, &starts) {
        let how = match &declared {
            Some(why) => format!("declared ({why})"),
            None => "undeclared".into(),
        };
        c.lines
            .push(format!("binary changed at session {k}: {how}"));
        if declared.is_none() {
            c.failed.push(format!(
                "study {id}: undeclared binary change at session {k}"
            ));
        }
    }
    done_lines(id, &all, &mut c);
    c
}

/// The `done` line, and a failure for each record after `done` that is not a `stamp` or an
/// `opened` record.
fn done_lines(id: &str, all: &[records::Record], c: &mut StudyCheck) {
    let Some(at) = all.iter().position(|r| r.kind == DONE) else {
        c.lines.push("no done record".into());
        return;
    };
    let seq = all[at].seq;
    match DoneRecord::of(&all[at]) {
        Some(d) => {
            let end = serde_json::to_value(d.end).ok();
            let end = end.as_ref().and_then(|v| v.as_str()).unwrap_or_default();
            c.lines
                .push(format!("done: {end} at {} (record {seq})", d.ended_at));
        }
        None => c.failed.push(format!(
            "study {id}: record {seq}: the done record is not readable"
        )),
    }
    for r in &all[at + 1..] {
        match r.kind.as_str() {
            STAMP | OPENED => {}
            DONE => c.failed.push(format!(
                "study {id}: record {} is a second done record",
                r.seq
            )),
            kind => c.failed.push(format!(
                "study {id}: record {} ({kind}) follows the done record",
                r.seq
            )),
        }
    }
}

fn study_prefix(id: &str) -> String {
    format!("study {id}: ")
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
    b.map_or_else(|| "not recorded".into(), |b| b.text())
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
