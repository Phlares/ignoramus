//! `register`: a person approves a checked plan; the box stores it, its payload and its
//! inputs, and logs it.
use super::check::{self, Assessed, PENDING};
use super::{CliError, Ctx, Done};
use crate::backend::output::file_digest;
use crate::canon::to_canonical;
use crate::derived::{REGISTER, Register, Resolved};
use crate::diag::render_human;
use crate::estimate::defaults_json;
use crate::files::{self, WriteOnceError, write_once};
use crate::home::Home;
use crate::inputs::{self, Found};
use crate::payload::{self, PayloadError};
use crate::planlog::store::{Leaf, PlanLog};
use crate::planlog::{commitment, family};
use crate::records::{self, RecordDraft};
use crate::rules::{self, Lookup, Taken};
use crate::spec::model::{Condition, Param, Spec, param_path};
use crate::spec::{self};
use crate::store::Store;
use crate::{keys, time};
use ed25519_dalek::SigningKey;
use serde_json::{Map, Value, json};
use std::io::{BufRead, IsTerminal, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

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

/// A refusal for a code that only the command line raises; the text comes from the rule
/// file. `found` says what this case found, when the rule text alone does not.
fn refusal(code: &str, found: &str) -> CliError {
    let text = rules::builtin()
        .iter()
        .find(|r| r.code == code)
        .map_or(String::new(), |r| {
            format!("{} {}", r.problem, r.instruction)
        });
    let found = if found.is_empty() {
        String::new()
    } else {
        format!(" Found: {found}.")
    };
    CliError::refuse(format!("{code}: {text}{found} See docs/rules/{code}.md"))
}

fn io(what: &str, e: impl std::fmt::Display) -> CliError {
    CliError::usage(format!("{what}: {e}"))
}

fn put(dir: &Path, name: &str, bytes: &[u8], mode: u32) -> Result<(), CliError> {
    write_once(&dir.join(name), bytes, mode).map_err(|e| match e {
        WriteOnceError::Differs | WriteOnceError::Link => refusal("E-REG-002", ""),
        WriteOnceError::Io(e) => io(name, e),
    })?;
    Ok(())
}

/// What `register` freezes besides the spec: the payload folder's hash, the digest of the
/// stored copy of each input present, and the values taken from other studies.
struct Frozen {
    payload: String,
    inputs: Vec<(String, String)>,
    resolved: Vec<Resolved>,
}

/// The body of the `register` record. A confirmatory plan waits for a timestamp, which a later version adds.
fn record_body(
    hash: &str,
    leaf: u64,
    log: &str,
    filled: &[(String, Value)],
    confirmatory: bool,
    root: &Path,
    frozen: &Frozen,
) -> Result<Value, CliError> {
    let root = root
        .to_str()
        .ok_or_else(|| CliError::usage("the study root is not valid text"))?;
    serde_json::to_value(Register {
        spec_hash: hash.into(),
        leaf,
        log: log.into(),
        defaults: defaults_json(filled),
        root: root.into(),
        payload: frozen.payload.clone(),
        inputs: frozen.inputs.clone(),
        resolved: frozen.resolved.clone(),
        timestamp: confirmatory.then(|| "pending".into()),
    })
    .map_err(|e| io("register record", e))
}

/// `register` on the real stdin and stdout.
pub fn run_stdio(ctx: &Ctx, spec_path: &Path) -> Result<Done, CliError> {
    let stdin = std::io::stdin();
    // The terminal check comes before anything else, the home included.
    let is_tty = stdin.is_terminal();
    if !is_tty {
        return Err(refusal("E-REG-001", ""));
    }
    run(
        spec_path,
        ctx.home()?,
        &mut stdin.lock(),
        &mut std::io::stdout().lock(),
        is_tty,
    )
}

/// Nothing named `studies/<id>` exists until every local write has succeeded. Every check
/// that can refuse comes first: the rules, the values taken from other studies, the payload
/// hash and its pin, the approval, the input imports, the spec re-read and the log tails. Then the study is staged under a
/// temporary name and published by one rename, and only the plan leaf and the `register`
/// record are appended after it.
pub fn run(
    spec_path: &Path,
    home: &Home,
    input: &mut impl BufRead,
    output: &mut impl Write,
    is_tty: bool,
) -> Result<Done, CliError> {
    if !is_tty {
        return Err(refusal("E-REG-001", ""));
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
        spec_hash: Some(a.hash.clone()),
        leaf,
        ..Done::exit(code)
    };
    if a.has_errors() {
        return Ok(done(1, None));
    }
    let spec = Spec::from_value(&a.value).map_err(|e| io("spec does not fit the schema", e))?;
    let resolved = resolve(&a, &spec.design.conditions)?;
    let id = spec.study.id.as_str();
    if home.study_dir(id).symlink_metadata().is_ok() {
        return Err(refusal("E-REG-002", ""));
    }
    let payload = payload_hash(&a.root, &spec)?;
    writeln!(
        output,
        "plan {id}: {}, earnable label {}, spec {}, payload sha256:{payload}",
        mode_of(&a),
        label_of(&a),
        a.hash
    )
    .map_err(say)?;
    for r in &resolved {
        writeln!(output, "{}", taken(r)).map_err(say)?;
    }
    write!(output, "{PROMPT} ").map_err(say)?;
    output.flush().map_err(say)?;
    let mut typed = String::new();
    input.read_line(&mut typed).map_err(|e| io("stdin", e))?;
    if !approve(&typed, &a.hash) {
        return Err(CliError::refuse("approval not given; nothing was written"));
    }
    let key = keys::load_or_create(home).map_err(|e| io("box key", e))?;
    let blobs = Store::open(&home.root).map_err(|e| io("store", e))?;
    let inputs = hold_inputs(&blobs, &key, &spec, &a)?;
    let frozen = Frozen {
        payload,
        inputs,
        resolved,
    };
    let leaf = store(spec_path, home, &spec, &a, &key, &frozen)?;
    writeln!(output, "registered {id}: leaf {leaf}").map_err(say)?;
    Ok(done(0, Some(leaf)))
}

/// The values the plan takes from other studies: for each hypothesis with a claim in a
/// confirmatory plan, the sd and icc per group and the estimate records they come from; for
/// each condition param that names a study, the condition that study selected and its
/// `selection` record. A value still pending refuses, naming the study to wait for, and so
/// does one the box no longer finds as the check found it.
fn resolve(a: &Assessed, conditions: &[Condition]) -> Result<Vec<Resolved>, CliError> {
    let hypotheses = a.value["question"]["hypotheses"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let claimed = || {
        hypotheses
            .iter()
            .enumerate()
            .filter(|(_, h)| h.get("claim").is_some())
            .map(|(i, h)| {
                let path = format!("question.hypotheses[{i}].estimates");
                (i, path, h["estimates"]["from"].as_str().unwrap_or_default())
            })
    };
    let params: Vec<(String, &str)> = (conditions.iter().enumerate())
        .flat_map(|(i, c)| {
            c.params.iter().filter_map(move |(k, p)| match p {
                Param::Ref(r) => Some((param_path(i, k), r.from.as_str())),
                Param::Value(_) => None,
            })
        })
        .collect();
    let pending = a.pending();
    let mut waits: Vec<String> = Vec::new();
    let named = claimed().map(|(_, path, study)| (path, study));
    for (path, study) in named.chain(params.iter().map(|(p, s)| (p.clone(), *s))) {
        let wait = format!("register after {study} finishes");
        if pending.contains(&path.as_str()) && !waits.contains(&wait) {
            waits.push(wait);
        }
    }
    if !pending.is_empty() {
        return Err(CliError::refuse(format!(
            "{PENDING}: a value waits on a study that has not finished; {}. See docs/rules/{PENDING}.md",
            waits.join("; ")
        )));
    }
    let drifted = |path: &str, why: &str| {
        CliError::refuse(format!(
            "{path} no longer reads as the check read it: {why} The study was not registered."
        ))
    };
    let mut out = Vec::new();
    for (i, path, study) in claimed().filter(|_| mode_of(a) == "confirmatory") {
        let groups = match rules::estimate_inputs(&a.value, i, &a.ctx) {
            Lookup::Found(groups) => groups,
            Lookup::Pending(why) | Lookup::Missing(why) => return Err(drifted(&path, &why)),
        };
        let mut records: Vec<String> = Vec::new();
        let mut value = Map::new();
        for g in groups {
            for r in g.records {
                if !records.contains(&r) {
                    records.push(r);
                }
            }
            let mut taken = json!({ "sd": g.sd, "icc": g.icc });
            if g.pooled {
                taken["pooled"] = json!(true);
            }
            value.insert(g.stratum, taken);
        }
        out.push(Resolved {
            path,
            study: study.into(),
            records,
            value: Value::Object(value),
        });
    }
    for (path, study) in params {
        match rules::selected_by(&a.ctx, study) {
            Taken::Found { selected, record } => out.push(Resolved {
                path,
                study: study.into(),
                records: vec![record],
                value: json!(selected),
            }),
            Taken::Pending(why) | Taken::Refused(why) => return Err(drifted(&path, &why)),
        }
    }
    Ok(out)
}

/// `takes <path> from <study>: stratum <name> sd <sd>, icc <icc>; …`, the values a person
/// approves with the plan.
fn taken(r: &Resolved) -> String {
    if let Some(condition) = r.value.as_str() {
        return format!("takes {} from {}: condition {condition}", r.path, r.study);
    }
    let groups: Vec<String> = r
        .value
        .as_object()
        .into_iter()
        .flatten()
        .map(|(stratum, v)| {
            let group = if stratum.is_empty() {
                "the pooled group".to_string()
            } else {
                format!("stratum {stratum}")
            };
            let num = |k: &str| v[k].as_f64().map_or("?".into(), |x| x.to_string());
            let from = if v["pooled"] == true && !stratum.is_empty() {
                " (the pooled estimate)"
            } else {
                ""
            };
            format!("{group} sd {}, icc {}{from}", num("sd"), num("icc"))
        })
        .collect();
    format!("takes {} from {}: {}", r.path, r.study, groups.join("; "))
}

fn mode_of(a: &Assessed) -> &str {
    a.value["study"]["mode"].as_str().unwrap_or_default()
}

fn label_of(a: &Assessed) -> &str {
    a.estimate
        .as_ref()
        .map_or("", |e| e.label_earnable.as_str())
}

/// The tree hash of the payload folder under the study root. E-REG-003 when there is no
/// folder, `payload` is a file, there is no executable `payload/run`, the folder holds a
/// link or another non-regular file, or `pins.payload` is set to another hash.
fn payload_hash(root: &Path, spec: &Spec) -> Result<String, CliError> {
    let dir = root.join(payload::DIR);
    let found = |what: &str| refusal("E-REG-003", what);
    match dir.symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(found("the study root has no payload folder"));
        }
        Ok(m) if m.is_file() => return Err(found("payload is a file, not a folder")),
        _ => {}
    }
    let hash = payload::tree_hash(&dir).map_err(|e| match e {
        PayloadError::Io(e) => io("payload", e),
        e => found(&e.to_string()),
    })?;
    let run = dir.join("run").metadata().map_err(|e| io("payload", e))?;
    if run.permissions().mode() & 0o111 == 0 {
        return Err(found("payload/run is not executable"));
    }
    let pin = spec.pins.payload.trim();
    let pin = pin.strip_prefix("sha256:").unwrap_or(pin);
    if !pin.is_empty() && pin != hash {
        return Err(found(&format!(
            "pins.payload is {pin}, and the folder hashes to {hash}"
        )));
    }
    Ok(hash)
}

/// Bring every input into the store, and check that each still holds the bytes the check
/// hashed. A pin the store knows names a blob, which is hashed again in full, once. Any
/// other input is the regular file under the root, imported in one streamed read and signed
/// the first time the store sees it. A store fault is an I/O error, not a changed input.
/// Returns the path and digest of each input present.
fn hold_inputs(
    store: &Store,
    key: &SigningKey,
    spec: &Spec,
    a: &Assessed,
) -> Result<Vec<(String, String)>, CliError> {
    let source = format!("register {}", spec.study.id);
    let mut held = Vec::new();
    for (input, (path, checked)) in spec.inputs.iter().zip(&a.input_hashes) {
        let now = match inputs::stored(store, &input.sha256) {
            Some(Found::Stored { digest, meta }) => {
                let blob = store.path(&digest).map_err(|e| io("store", e))?;
                let same = file_digest(&blob).ok().flatten().as_ref() == Some(&digest);
                same.then_some((digest, meta.normalized_sha256))
            }
            Some(_) => None,
            None => match inputs::regular_file(&a.root, path) {
                Some(file) => {
                    let (digest, meta) = store
                        .import_signed(&file, key, &source)
                        .map_err(|e| io("store", e))?;
                    Some((digest, meta.normalized_sha256))
                }
                None => None,
            },
        };
        // A pin cannot differ here: E-HSH-001 tied the checked hash to it.
        if now != *checked {
            return Err(CliError::refuse(format!(
                "input {path} changed or became unreadable since the check; \
                 the study was not registered"
            )));
        }
        if let Some((digest, _)) = now {
            held.push((path.clone(), digest));
        }
    }
    Ok(held)
}

/// Re-read the spec and check both log tails, stage and publish the study folder, then
/// append the plan leaf and the `register` record. Another process can register the same id
/// between the check and the publish; the rename refuses it, before any leaf is appended. A
/// fault after the rename is an I/O fault in an append and leaves a folder without a record.
/// The log appends are not atomic with the earlier-use lookup in `check`; v0 accepts that.
/// `check_tails` runs before `append` takes its lock.
fn store(
    spec_path: &Path,
    home: &Home,
    spec: &Spec,
    a: &Assessed,
    key: &SigningKey,
    frozen: &Frozen,
) -> Result<u64, CliError> {
    let bytes = std::fs::read(spec_path).map_err(|e| io("cannot read spec", e))?;
    let raw = spec::parse(&bytes).map_err(|e| io("cannot read spec", e))?;
    if raw.hash != a.hash {
        return Err(CliError::refuse("the spec changed after it was checked"));
    }
    let canon = to_canonical(&raw.value).map_err(|e| io("canonical form", e))?;
    let id = spec.study.id.as_str();
    let mode = mode_of(a);
    let log = if spec.study.test_log {
        "testlog"
    } else {
        "planlog"
    };
    let plan_log = PlanLog::open(&home.root.join(log))
        .and_then(|l| l.check_tails().map(|()| l))
        .map_err(|e| io(log, e))?;
    let dir = home.study_dir(id);
    let (staged, salt) = stage(&dir, a, &bytes, &canon, frozen)?;
    std::fs::rename(&staged, &dir).map_err(|e| {
        let kept = kept(&staged);
        match e.raw_os_error() {
            Some(libc::ENOTEMPTY | libc::EEXIST | libc::ENOTDIR) => refusal("E-REG-002", &kept),
            _ => io("cannot publish the study folder", format!("{e}; {kept}")),
        }
    })?;
    // Only appends from here: a lost rename above appended no leaf.
    let data_hashes = a
        .input_hashes
        .iter()
        .filter_map(|(_, h)| h.clone())
        .collect();
    let (leaf, _) = plan_log
        .append(
            Leaf {
                kind: "plan".into(),
                commitment: Some(commitment(&salt, &canon)),
                family: Some(family(spec)),
                mode: Some(mode.into()),
                data_hashes,
                label: label_of(a).into(),
                ts: time::now_rfc3339(),
            },
            key,
        )
        .map_err(|e| io(log, e))?;
    records::append(
        &dir.join("records.jsonl"),
        key,
        RecordDraft {
            study: id.into(),
            row: None,
            kind: REGISTER.into(),
            body: record_body(
                &a.hash,
                leaf,
                log,
                &a.filled,
                mode == "confirmatory",
                &a.root,
                frozen,
            )?,
        },
    )
    .map_err(|e| io("records.jsonl", e))?;
    Ok(leaf)
}

/// What a refusal after staging says: no study, and where the staged files are.
fn kept(staged: &Path) -> String {
    format!(
        "the study was not registered, and {} is kept",
        staged.display()
    )
}

/// Write the spec files, the salt and the sealed payload copy into a new private folder
/// beside the study folder (`studies/.<id>.<pid>.<random>.tmp`), and check that the copy is
/// the payload approved at the prompt. A study id never starts with `.`, and no reader
/// takes a folder without `records.jsonl` for a study. Returns the folder and the salt.
fn stage(
    dir: &Path,
    a: &Assessed,
    bytes: &[u8],
    canon: &[u8],
    frozen: &Frozen,
) -> Result<(PathBuf, [u8; 32]), CliError> {
    let staged = files::temp_beside(dir).map_err(|e| io("staging folder", e))?;
    let mut mk = std::fs::DirBuilder::new();
    std::os::unix::fs::DirBuilderExt::mode(&mut mk, 0o700);
    mk.create(&staged).map_err(|e| io("staging folder", e))?;
    let fill = || -> Result<[u8; 32], CliError> {
        let mut salt = [0u8; 32];
        getrandom::fill(&mut salt).map_err(|e| io("random bytes", e))?;
        put(&staged, "spec.toml", bytes, 0o644)?;
        put(&staged, "spec.json", canon, 0o644)?;
        put(
            &staged,
            "spec.hash",
            format!("{}\n", a.hash).as_bytes(),
            0o644,
        )?;
        put(&staged, "salt", &salt, 0o600)?;
        let copied = payload::snapshot(&a.root, &staged).map_err(|e| match e {
            PayloadError::Io(e) => io("payload copy", e),
            e => refusal("E-REG-003", &e.to_string()),
        })?;
        if copied != frozen.payload {
            return Err(CliError::refuse("the payload changed after the check"));
        }
        Ok(salt)
    };
    match fill() {
        Ok(salt) => Ok((staged, salt)),
        Err(mut e) => {
            e.msg = format!("{}; {}", e.msg, kept(&staged));
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_confirmatory_record_is_pending() {
        let filled = [("design.alpha".to_string(), json!(0.005))];
        let root = Path::new("/work/study");
        let frozen = Frozen {
            payload: "ab".repeat(32),
            inputs: vec![("data/x.txt".into(), "cd".repeat(32))],
            resolved: vec![Resolved {
                path: "question.hypotheses[0].estimates".into(),
                study: "pilot".into(),
                records: vec!["ef".repeat(32)],
                value: json!({ "": { "sd": 2.0, "icc": 0.1 } }),
            }],
        };
        let exp = record_body("sha256:ab", 3, "planlog", &filled, false, root, &frozen).unwrap();
        assert_eq!(
            exp,
            json!({"spec_hash": "sha256:ab", "leaf": 3, "log": "planlog",
                   "defaults": [{"path": "design.alpha", "value": 0.005}],
                   "root": "/work/study", "payload": "ab".repeat(32),
                   "inputs": [["data/x.txt", "cd".repeat(32)]],
                   "resolved": [{ "path": "question.hypotheses[0].estimates",
                                  "study": "pilot", "records": ["ef".repeat(32)],
                                  "value": { "": { "sd": 2.0, "icc": 0.1 } } }]})
        );
        let conf = record_body("sha256:ab", 3, "testlog", &filled, true, root, &frozen).unwrap();
        assert_eq!(conf["timestamp"], "pending");
        assert_eq!(conf["log"], "testlog");
    }

    #[test]
    fn a_value_the_box_no_longer_holds_refuses() {
        // A plan the check passed, read again against a box without the study it names.
        let value = json!({
            "study": { "id": "s", "mode": "confirmatory" },
            "question": { "hypotheses": [{
                "outcome": "o", "contrast": { "arm-a": 1.0 }, "claim": { "below": 0.0 },
                "estimates": { "from": "pilot" }
            }] },
            "outcomes": [{ "id": "o", "unit": "u", "direction": "lower", "measure": "m",
                           "paired": "difference", "registered": true }],
            "design": { "reference": "exact", "conditions": [{ "id": "exact" }, { "id": "arm-a" }],
                        "confirmation": { "conditions": ["arm-a"] } }
        });
        let root = Path::new("/work/study");
        let a = Assessed {
            root: root.into(),
            hash: "sha256:ab".into(),
            value,
            filled: vec![],
            diags: vec![],
            estimate: None,
            input_hashes: vec![],
            ctx: rules::CheckCtx::empty(root),
        };
        let e = resolve(&a, &[]).unwrap_err();
        assert_eq!(e.code, 1);
        assert_eq!(
            e.msg,
            "question.hypotheses[0].estimates no longer reads as the check read it: Waits on study pilot, which is not registered. The study was not registered."
        );
    }
}
