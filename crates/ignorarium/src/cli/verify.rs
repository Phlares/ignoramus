//! `verify`: check the plan logs and, for one study, its record chain; or check a copy of a
//! study's log with a public key alone.
use super::{CliError, Ctx, Done, Run, key, out};
use crate::derived::{
    By, DONE, DoneRecord, OPENED, Register, STAMP, SessionStart, Stamp, first_done, sessions,
    stamp_flag,
};
use crate::diag::printable;
use crate::identity::same_binary;
use crate::planlog::store::PlanLog;
use crate::records::Seal;
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
    fail_or_pass(finish(check_study(None, records, &key))?)
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
            failed.extend(finish(check)?);
        }
    }
    fail_or_pass(failed)
}

/// Print what a study check found, and return its failures. Each line is escaped: with a
/// public key, the signer under check chose every string in it.
fn finish(check: StudyCheck) -> Result<Vec<String>, CliError> {
    for line in &check.lines {
        out(&format!("{}\n", printable(line)))?;
    }
    Ok(check.failed)
}

/// Exit 0 when nothing failed; otherwise a refusal that lists every failure, escaped.
fn fail_or_pass(failed: Vec<String>) -> Run {
    if failed.is_empty() {
        Ok(Done::exit(0))
    } else {
        let failed: Vec<String> = failed.iter().map(|f| printable(f)).collect();
        Err(CliError::refuse(format!(
            "verify failed: {}",
            failed.join("; ")
        )))
    }
}

/// What one study's log shows: the lines to print and the failures found.
struct StudyCheck {
    lines: Vec<String>,
    failed: Vec<String>,
}

/// Check the chain of the log at `log` with `key`, and what it holds. `id` names the study in
/// the lines; without it the study is the one the first record names.
fn check_study(id: Option<&str>, log: &Path, key: &VerifyingKey) -> StudyCheck {
    let mut c = StudyCheck {
        lines: Vec::new(),
        failed: Vec::new(),
    };
    let prefix = id.map_or(String::new(), |id| format!("study {id}: "));
    let read = records::read_all(log, key).and_then(|all| Ok((all, records::seals(log, key)?)));
    let (all, seals) = match read {
        Ok(read) => read,
        Err(e) => {
            c.failed.push(format!("{prefix}{e}"));
            return c;
        }
    };
    let (Some(first), Some(sealed)) = (all.first(), seals.first()) else {
        c.failed.push(format!("{prefix}no register record"));
        return c;
    };
    let id = id.unwrap_or(&first.study);
    c.lines.push(format!("study {id}: {} records", all.len()));
    c.lines
        .push(format!("register record hash: {}", sealed.hash));
    // The register is the first record of the log.
    let registered = Register::of(first).and_then(|r| r.box_identity);
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
    done_lines(id, &all, &seals, &mut c);
    stamp_lines(id, &all, &seals, &mut c);
    c
}

/// A failure unless `by`, which record `seq` (a `kind` record) names, is a record before it
/// with that stored hash.
fn names_earlier(id: &str, seals: &[Seal], seq: u64, kind: &str, by: &By) -> Option<String> {
    // A checked log numbers its records from 0, so `seals[i]` is record `i`.
    let earlier = usize::try_from(seq).ok().and_then(|n| seals.get(..n));
    let seal = usize::try_from(by.seq).ok().and_then(|i| earlier?.get(i));
    let held = seal.is_some_and(|s| s.hash == by.hash);
    (!held).then(|| {
        format!(
            "study {id}: record {seq}: the {kind} record names no earlier record {} with hash {}",
            by.seq, by.hash
        )
    })
}

/// One line for each `stamp` record, and a flag line for each stamp dated before a record it
/// covers. A flag does not fail the check; an unreadable stamp, or a head the log does not
/// hold, does.
fn stamp_lines(id: &str, all: &[records::Record], seals: &[Seal], c: &mut StudyCheck) {
    let yes = |b: bool| if b { "yes" } else { "no" };
    for r in all.iter().filter(|r| r.kind == STAMP) {
        let Some(s) = Stamp::of(r) else {
            c.failed.push(format!(
                "study {id}: record {}: the stamp record is not readable",
                r.seq
            ));
            continue;
        };
        let l = &s.lines;
        let head = (l.head.as_ref()).map_or("none".into(), |h| format!("{} {}", h.seq, h.hash));
        let named = l
            .head
            .as_ref()
            .and_then(|h| names_earlier(id, seals, r.seq, STAMP, h));
        c.failed.extend(named);
        c.lines.push(format!(
            "stamp {}: {}, {}; lines: register {}, signature {}, key {}, head {head}, head signature {}; file {}, reply {}",
            r.seq,
            s.stamped_at,
            s.authority.as_deref().unwrap_or("authority not stated"),
            yes(l.register),
            yes(l.register_signature),
            yes(l.key),
            yes(l.head_signature),
            s.file_sha256,
            s.reply_sha256,
        ));
        if let Some(flag) = stamp_flag(all, &s) {
            c.lines.push(format!("flag: stamp {} is {flag}", r.seq));
        }
    }
}

/// The `done` line, a failure when its `by` is not an earlier record of the log, and a failure
/// for each record after `done` that is not a `stamp` or an `opened` record.
fn done_lines(id: &str, all: &[records::Record], seals: &[Seal], c: &mut StudyCheck) {
    let Some(done) = first_done(all) else {
        c.lines.push("no done record".into());
        return;
    };
    let seq = done.seq;
    match DoneRecord::of(done) {
        Some(d) => {
            let end = serde_json::to_value(d.end).ok();
            let end = end.as_ref().and_then(|v| v.as_str()).unwrap_or_default();
            c.lines
                .push(format!("done: {end} at {} (record {seq})", d.ended_at));
            c.failed.extend(names_earlier(id, seals, seq, DONE, &d.by));
        }
        None => c.failed.push(format!(
            "study {id}: record {seq}: the done record is not readable"
        )),
    }
    for r in all.iter().filter(|r| r.seq > seq) {
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
