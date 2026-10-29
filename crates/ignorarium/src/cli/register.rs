//! `register`: a person approves a checked plan; the box stores it and logs it.
use super::check::{self, Assessed};
use super::{CliError, Ctx, Done};
use crate::canon::to_canonical;
use crate::diag::render_human;
use crate::estimate::defaults_json;
use crate::files::{WriteOnceError, write_once};
use crate::home::Home;
use crate::planlog::store::{Leaf, PlanLog};
use crate::planlog::{commitment, family};
use crate::prior::file_hashes;
use crate::records::{self, RecordDraft};
use crate::rules::{self, input_bytes};
use crate::spec::{self, model::Spec};
use crate::{keys, time};
use serde_json::{Value, json};
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;

const PROMPT: &str = "Type the first 8 characters of the spec hash to approve:";

/// True when `typed` is exactly the first 8 lowercase hex characters after `sha256:`.
pub fn approve(typed: &str, spec_hash: &str) -> bool {
    let typed = typed.trim();
    spec_hash
        .strip_prefix("sha256:")
        .and_then(|h| h.get(..8))
        .is_some_and(|p| {
            typed == p
                && p.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

/// A refusal for a code that only the command line raises; the text comes from the rule file.
fn refusal(code: &str) -> CliError {
    let text = rules::builtin()
        .iter()
        .find(|r| r.code == code)
        .map_or(String::new(), |r| {
            format!("{} {}", r.problem, r.instruction)
        });
    CliError::refuse(format!("{code}: {text} See docs/rules/{code}.md"))
}

fn io(what: &str, e: impl std::fmt::Display) -> CliError {
    CliError::usage(format!("{what}: {e}"))
}

fn put(dir: &Path, name: &str, bytes: &[u8], mode: u32) -> Result<(), CliError> {
    write_once(&dir.join(name), bytes, mode).map_err(|e| match e {
        WriteOnceError::Differs | WriteOnceError::Link => refusal("E-REG-002"),
        WriteOnceError::Io(e) => io(name, e),
    })?;
    Ok(())
}

/// The body of the `register` record. A confirmatory plan waits for a stamp (Plan 3).
fn record_body(
    hash: &str,
    leaf: u64,
    log: &str,
    filled: &[(String, Value)],
    confirmatory: bool,
) -> Value {
    let mut body = json!({
        "spec_hash": hash,
        "leaf": leaf,
        "log": log,
        "defaults": defaults_json(filled),
    });
    if confirmatory {
        body["timestamp"] = json!("pending");
    }
    body
}

/// `register` on the real stdin and stdout.
pub fn run_stdio(ctx: &Ctx, spec_path: &Path) -> Result<Done, CliError> {
    let stdin = std::io::stdin();
    // The terminal check comes before anything else, the home included.
    let is_tty = stdin.is_terminal();
    if !is_tty {
        return Err(refusal("E-REG-001"));
    }
    run(
        spec_path,
        ctx.home()?,
        &mut stdin.lock(),
        &mut std::io::stdout().lock(),
        is_tty,
    )
}

pub fn run(
    spec_path: &Path,
    home: &Home,
    input: &mut impl BufRead,
    output: &mut impl Write,
    is_tty: bool,
) -> Result<Done, CliError> {
    if !is_tty {
        return Err(refusal("E-REG-001"));
    }
    let ctx = Ctx {
        home: Ok(home.clone()),
    };
    // `assess` reads the plan log and finishes before `append` below takes its lock.
    let a = check::assess(&ctx, spec_path)?;
    let say = |e| io("cannot write output", e);
    if !a.diags.is_empty() {
        writeln!(output, "{}", render_human(&a.diags)).map_err(say)?;
    }
    let done = |code, leaf| Done {
        code,
        spec_hash: Some(a.hash.clone()),
        leaf,
    };
    if a.has_errors() {
        return Ok(done(1, None));
    }
    let spec = Spec::from_value(&a.value).map_err(|e| io("spec does not fit the schema", e))?;
    let id = spec.study.id.as_str();
    if home.study_dir(id).symlink_metadata().is_ok() {
        return Err(refusal("E-REG-002"));
    }
    let label = a
        .estimate
        .as_ref()
        .map_or("", |e| e.label_earnable.as_str());
    let mode = a.value["study"]["mode"].as_str().unwrap_or_default();
    writeln!(
        output,
        "plan {id}: {mode}, earnable label {label}, spec {}",
        a.hash
    )
    .map_err(say)?;
    write!(output, "{PROMPT} ").map_err(say)?;
    output.flush().map_err(say)?;
    let mut typed = String::new();
    input.read_line(&mut typed).map_err(|e| io("stdin", e))?;
    if !approve(&typed, &a.hash) {
        return Err(CliError::refuse("approval not given; nothing was written"));
    }
    // An input that changed since the check would make the stored hashes a lie.
    let study_dir = check::study_dir(spec_path);
    for (path, checked) in &a.input_hashes {
        let now = input_bytes(&study_dir, path).map(|b| file_hashes(&b));
        if &now != checked {
            return Err(CliError::refuse(format!(
                "input {path} changed or became unreadable since the check; nothing was written"
            )));
        }
    }
    let leaf = store(spec_path, home, &spec, &a, label, mode)?;
    writeln!(output, "registered {id}: leaf {leaf}").map_err(say)?;
    Ok(done(0, Some(leaf)))
}

/// Write the study folder, then the plan leaf, then the `register` record.
/// Another process can register the same id between the check and the
/// `create_dir`; the `create_dir` refuses it. The log appends are not atomic with
/// the earlier-use lookup in `check`; v0 accepts that.
fn store(
    spec_path: &Path,
    home: &Home,
    spec: &Spec,
    a: &Assessed,
    label: &str,
    mode: &str,
) -> Result<u64, CliError> {
    let bytes = std::fs::read(spec_path).map_err(|e| io("cannot read spec", e))?;
    let raw = spec::parse(&bytes).map_err(|e| io("cannot read spec", e))?;
    if raw.hash != a.hash {
        return Err(CliError::refuse("the spec changed after it was checked"));
    }
    let canon = to_canonical(&raw.value).map_err(|e| io("canonical form", e))?;
    let id = spec.study.id.as_str();
    let dir = home.study_dir(id);
    let mut mk = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut mk, 0o700);
    mk.create(&dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => refusal("E-REG-002"),
        _ => io("cannot create the study folder", e),
    })?;
    let mut salt = [0u8; 32];
    getrandom::fill(&mut salt).map_err(|e| io("random bytes", e))?;
    put(&dir, "spec.toml", &bytes, 0o644)?;
    put(&dir, "spec.json", &canon, 0o644)?;
    put(&dir, "spec.hash", format!("{}\n", a.hash).as_bytes(), 0o644)?;
    put(&dir, "salt", &salt, 0o600)?;

    let data_hashes = a
        .input_hashes
        .iter()
        .filter_map(|(_, h)| h.clone())
        .collect();
    let log = if spec.study.test_log {
        "testlog"
    } else {
        "planlog"
    };
    let key = keys::load_or_create(home).map_err(|e| io("box key", e))?;
    let (leaf, _) = PlanLog::open(&home.root.join(log))
        .and_then(|l| {
            l.append(
                Leaf {
                    kind: "plan".into(),
                    commitment: Some(commitment(&salt, &canon)),
                    family: Some(family(spec)),
                    mode: Some(mode.into()),
                    data_hashes,
                    label: label.into(),
                    ts: time::now_rfc3339(),
                },
                &key,
            )
        })
        .map_err(|e| io(log, e))?;
    records::append(
        &dir.join("records.jsonl"),
        &key,
        RecordDraft {
            study: id.into(),
            row: None,
            kind: "register".into(),
            body: record_body(&a.hash, leaf, log, &a.filled, mode == "confirmatory"),
        },
    )
    .map_err(|e| io("records.jsonl", e))?;
    Ok(leaf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_confirmatory_record_is_pending() {
        let filled = [("design.alpha".to_string(), json!(0.005))];
        let exp = record_body("sha256:ab", 3, "planlog", &filled, false);
        assert_eq!(
            exp,
            json!({"spec_hash": "sha256:ab", "leaf": 3, "log": "planlog",
                   "defaults": [{"path": "design.alpha", "value": 0.005}]})
        );
        let conf = record_body("sha256:ab", 3, "testlog", &filled, true);
        assert_eq!(conf["timestamp"], "pending");
        assert_eq!(conf["log"], "testlog");
    }
}
