//! `check`: load a spec, run every rule, and report. Shared with `review`.
use super::{CliError, Ctx, Done, Run, out};
use crate::derived::{Alpha, Register};
use crate::diag::{Diagnostic, Severity, render_human};
use crate::estimate::{Estimate, defaults_json, estimate};
use crate::home::Home;
use crate::inputs;
use crate::keys;
use crate::planlog::store::PlanLog;
use crate::prior::{PriorError, PriorUse};
use crate::records;
use crate::rules::{self, BoxRecords, CheckCtx, ReadOnce};
use crate::schedule;
use crate::spec::{self, LoadError, model::Spec};
use crate::store::Store;
use crate::stream::{self, Log, LogName, Next, StreamError};
use ed25519_dalek::VerifyingKey;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};

/// Everything a check finds about one spec file.
pub struct Assessed {
    /// The study root the checks read from.
    pub root: PathBuf,
    /// Hash of the author's file, never of the defaulted value.
    pub hash: String,
    /// The spec with defaults filled.
    pub value: Value,
    pub filled: Vec<(String, Value)>,
    pub diags: Vec<Diagnostic>,
    /// Present only when the plan has no errors.
    pub estimate: Option<Estimate>,
    /// `(path, (exact, normalized) SHA-256)` of each input, read once here; `None`
    /// when the file is absent or its path is unsafe. `register` stores these.
    pub input_hashes: Vec<(String, Option<(String, String)>)>,
    /// What the rules read, the box's records among them; `register` resolves from it.
    pub ctx: CheckCtx,
    /// For a confirmatory plan, the slot and level the stream would give it now.
    pub stream: Option<Alpha>,
}

/// The code of a value that waits on a study that has not finished.
pub const PENDING: &str = "W-REF-001";

impl Assessed {
    pub fn has_errors(&self) -> bool {
        self.diags.iter().any(|d| d.severity == Severity::Error)
    }

    /// The paths of the values that wait on a study that has not finished.
    pub fn pending(&self) -> Vec<&str> {
        self.diags
            .iter()
            .filter(|d| d.code == PENDING)
            .map(|d| d.path.as_str())
            .collect()
    }

    /// 1 with an error, 3 with no error and a value pending, else 0.
    pub fn exit_code(&self) -> i32 {
        if self.has_errors() {
            1
        } else if !self.pending().is_empty() {
            3
        } else {
            0
        }
    }
}

/// A plan log that cannot be opened: every lookup fails, so the rules refuse.
struct Unreadable(String);

impl PriorUse for Unreadable {
    fn used(&self, _: &str, _: &str) -> Result<Option<String>, PriorError> {
        Err(PriorError(self.0.clone()))
    }
}

/// The records of the box in `home`. `key` is the box key, or why it cannot be read.
/// A study with no log needs no key; a log that no key can verify cannot be trusted.
pub struct HomeRecords {
    pub home: Home,
    pub key: Result<Option<VerifyingKey>, String>,
}

impl BoxRecords for HomeRecords {
    /// A study is registered when its log holds a `register` record. An id that is not a
    /// study id names no study, and no path is built from it. Only a log that does not exist
    /// means "not registered": any other fault, and a record of another study in the log,
    /// is an error.
    fn study(&self, id: &str) -> Result<Option<Vec<records::Record>>, String> {
        if !rules::is_study_id(id) {
            return Ok(None);
        }
        let log = self.home.study_dir(id).join("records.jsonl");
        let Some(key) = self.key_for(&log)? else {
            return Ok(None);
        };
        let all = records::read_all(&log, key).map_err(|e| e.to_string())?;
        if let Some(r) = all.iter().find(|r| r.study != id) {
            return Err(format!("record {} belongs to study {}", r.seq, r.study));
        }
        let registered = all.iter().any(|r| Register::of(r).is_some());
        Ok(registered.then_some(all))
    }

    /// A stream with no log is empty and needs no key. Each slot a signed `register` record
    /// of the box names must be in the stream, so a stream log that was moved, cut or rolled
    /// back is an error, never a fresh budget.
    fn stream_next(&self, log: &str) -> Result<Next, String> {
        let name = LogName::parse(log).map_err(|e| e.to_string())?;
        let stream = Log::of(&self.home, name);
        let all = match self.key_for(stream.path())? {
            None => vec![],
            Some(key) => records::read_all(stream.path(), key)
                .map_err(|e| StreamError::from(e).to_string())?,
        };
        stream::next_of(name, &all, &self.held()?).map_err(|e| e.to_string())
    }

    fn stream_closed(&self, log: &str, study: &str, slot: u64) -> Result<bool, String> {
        let name = LogName::parse(log).map_err(|e| e.to_string())?;
        let stream = Log::of(&self.home, name);
        match self.key_for(stream.path())? {
            None => Ok(false),
            Some(key) => stream.closed(key, study, slot).map_err(|e| e.to_string()),
        }
    }
}

impl HomeRecords {
    /// The stream slot each study of the box holds, by study, from the `register` record that
    /// opens its log. Only that first line is read and checked.
    fn held(&self) -> Result<Vec<(String, Alpha)>, String> {
        let mut out = Vec::new();
        for (id, log) in records::logs(&self.home).map_err(|e| e.to_string())? {
            let fault = |e: String| format!("study {id}: {e}");
            let Some(key) = self.key_for(&log).map_err(fault)? else {
                continue;
            };
            let first = records::first(&log, key).map_err(|e| fault(e.to_string()))?;
            let alpha = first.as_ref().and_then(Register::of).and_then(|r| r.alpha);
            if let (Some(r), Some(alpha)) = (first, alpha) {
                out.push((r.study, alpha));
            }
        }
        Ok(out)
    }

    /// The key that verifies the log at `path`; `None` when there is no log. Only a log that
    /// does not exist needs no key: any other fault is an error.
    fn key_for(&self, path: &Path) -> Result<Option<&VerifyingKey>, String> {
        match path.symlink_metadata() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.to_string()),
            Ok(_) => {}
        }
        match &self.key {
            Err(e) => Err(e.clone()),
            Ok(None) => Err("the box key is missing".into()),
            Ok(Some(key)) => Ok(Some(key)),
        }
    }
}

/// Hash every input the spec lists, from the store when the pin names a stored object.
/// This is the one read `register` relies on.
pub fn input_hashes(
    store: &Store,
    root: &Path,
    spec: &Value,
) -> Vec<(String, Option<(String, String)>)> {
    let list = spec["inputs"].as_array().map_or(&[][..], Vec::as_slice);
    list.iter()
        .filter_map(|i| Some((i["path"].as_str()?, i["sha256"].as_str().unwrap_or(""))))
        .map(|(p, sha)| {
            let found = inputs::resolve(store, root, p, sha);
            (p.to_string(), inputs::hashes(&found))
        })
        .collect()
}

/// The file whose presence marks a study root.
pub const ROOT_MARKER: &str = "ignorarium.toml";

/// The folder every study path is relative to: the nearest folder at or above the spec's
/// own folder that holds a regular `ignorarium.toml` (a link or folder of that name does
/// not count; the content is not read). The walk checks the first folder that holds a
/// `.git` entry and goes no further. It never uses a marker in the user's home folder or
/// the filesystem root. Without a marker, the spec's folder.
pub fn study_root(spec_path: &Path) -> PathBuf {
    study_root_in(spec_path, std::env::home_dir().as_deref())
}

/// `study_root` for a given home folder.
pub fn study_root_in(spec_path: &Path, home: Option<&Path>) -> PathBuf {
    let spec = std::path::absolute(spec_path).unwrap_or_else(|_| spec_path.to_path_buf());
    let spec = fold_dots(&spec);
    let folder = spec
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    for dir in folder.ancestors() {
        if dir.parent().is_none() || Some(dir) == home {
            break;
        }
        let has = |name: &str| dir.join(name).symlink_metadata();
        if has(ROOT_MARKER).is_ok_and(|m| m.is_file()) {
            return dir.to_path_buf();
        }
        if has(".git").is_ok() {
            break;
        }
    }
    folder
}

/// Drop `.` and fold `..` against the segment before it, without touching the disk, so the
/// walk climbs real parents only.
fn fold_dots(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

pub fn assess(ctx: &Ctx, spec_path: &Path) -> Result<Assessed, CliError> {
    let home = ctx.home()?;
    let shown = spec_path.display();
    let raw = spec::load(spec_path).map_err(|e| match e {
        LoadError::Io(_) => CliError::usage(format!("{shown}: {e}")),
        _ => CliError::refuse(format!("{shown}: {e}")),
    })?;
    let machine = crate::machine::load(&home.root).map_err(|e| CliError::usage(e.to_string()))?;
    let prior: Box<dyn PriorUse> = match PlanLog::open(&home.root.join("planlog")) {
        Ok(log) => Box::new(log),
        Err(e) => Box::new(Unreadable(e.to_string())),
    };
    let key = keys::load(home)
        .map(|k| k.map(|k| k.verifying_key()))
        .map_err(|e| e.to_string());
    // Not opened: a check writes nothing, and `meta` reads a missing folder as empty.
    let store = Store {
        root: home.root.join("store"),
    };
    let cc = CheckCtx {
        root: study_root(spec_path),
        prior,
        machine_max_level: machine.max_level,
        spec_text: raw.text,
        records: Box::new(ReadOnce::new(HomeRecords {
            home: home.clone(),
            key,
        })),
        store,
        spec_hash: spec::hash_bytes(&raw.hash)
            .ok_or_else(|| CliError::usage(format!("{shown}: the spec hash is malformed")))?,
        gates: Default::default(),
    };
    let (value, filled, diags) = rules::check_all(&raw.value, rules::builtin(), &cc);
    let mut a = Assessed {
        root: cc.root.clone(),
        hash: raw.hash,
        input_hashes: input_hashes(&cc.store, &cc.root, &value),
        filled,
        diags,
        estimate: None,
        stream: rules::granted(&value, &cc).ok().flatten(),
        value,
        ctx: cc,
    };
    if !a.has_errors() {
        let rows = Spec::from_value(&a.value).ok().and_then(|s| {
            let hash = spec::hash_bytes(&a.hash)?;
            Some((schedule::rows(&s, &hash).ok()?, s))
        });
        match rows {
            Some((rows, s)) => a.estimate = Some(estimate(&s, &machine, &rows)),
            None => a.diags.push(rules::schema_fault()),
        }
    }
    Ok(a)
}

/// `<id>: <n> rows`, one per condition.
pub fn condition_lines(e: &Estimate) -> Vec<String> {
    e.rows_per_condition
        .iter()
        .map(|(id, n)| format!("{id}: {n} rows"))
        .collect()
}

pub fn figure_lines(e: &Estimate) -> Vec<String> {
    vec![
        format!("rows: {}", e.rows),
        format!("cpu hours: {:.2}", e.cpu_hours),
        format!("wall hours: {:.2}", e.wall_hours),
        format!("money: ${:.2}", e.money_usd),
        format!("long job: {}", if e.is_long_job { "yes" } else { "no" }),
    ]
}

/// `stream: slot <n>, level <level>`.
pub fn stream_line(a: &Alpha) -> String {
    format!("stream: slot {}, level {}", a.slot, stream::shown(a.level))
}

pub fn default_lines(a: &Assessed) -> Vec<String> {
    a.filled
        .iter()
        .map(|(path, v)| format!("{path} = {v}"))
        .collect()
}

fn indented(lines: Vec<String>) -> String {
    lines.iter().map(|l| format!("  {l}\n")).collect()
}

fn render(a: &Assessed) -> String {
    let mut s = String::new();
    if !a.diags.is_empty() {
        s.push_str(&render_human(&a.diags));
        s.push('\n');
    }
    if let Some(e) = &a.estimate {
        s.push_str(&format!("conditions:\n{}\n", indented(condition_lines(e))));
        s.push_str(&format!("estimate:\n{}\n", indented(figure_lines(e))));
        s.push_str(&format!("label: {}\n\n", e.label_earnable));
        s.push_str(&format!("defaults:\n{}\n", indented(default_lines(a))));
    }
    if let Some(granted) = &a.stream {
        s.push_str(&format!("{}\n", stream_line(granted)));
    }
    s.push_str(&format!("spec: {}\n", a.hash));
    s
}

fn render_json(a: &Assessed) -> Result<String, CliError> {
    let estimate = a.estimate.as_ref().map(|e| {
        json!({
            "rows": e.rows,
            "rows_per_condition": e.rows_per_condition.iter()
                .map(|(id, rows)| json!({ "id": id, "rows": rows }))
                .collect::<Vec<_>>(),
            "cpu_hours": e.cpu_hours,
            "wall_hours": e.wall_hours,
            "money_usd": e.money_usd,
            "label": e.label_earnable,
            "long_job": e.is_long_job,
        })
    });
    let power: Vec<Value> = rules::power(&a.value, &a.ctx)
        .into_iter()
        .map(|p| {
            let clusters: serde_json::Map<String, Value> = (p.clusters.into_iter())
                .map(|(stratum, n)| (stratum, json!(n)))
                .collect();
            // `null` when the power reaches the target; a count, or `>1024` beyond the cap.
            let needed = match (p.power >= p.target, p.needed) {
                (true, _) => Value::Null,
                (false, Some(n)) => json!(n),
                (false, None) => json!(format!(">{}", rules::CAP)),
            };
            json!({
                "hypothesis": p.hypothesis,
                "power": p.power,
                "target": p.target,
                "clusters": clusters,
                "needed": needed,
            })
        })
        .collect();
    let v = json!({
        "diagnostics": a.diags,
        "pending": a.pending(),
        "power": power,
        "stream": a.stream,
        "estimate": estimate,
        "defaults": defaults_json(&a.filled),
        "hash": a.hash,
    });
    serde_json::to_string_pretty(&v).map_err(|e| CliError::usage(e.to_string()))
}

pub fn run(ctx: &Ctx, spec_path: &Path, json: bool) -> Run {
    let a = match assess(ctx, spec_path) {
        Err(e) if json => {
            let v = json!({ "diagnostics": [], "error": e.msg, "hash": null });
            let text =
                serde_json::to_string_pretty(&v).map_err(|e| CliError::usage(e.to_string()))?;
            out(&format!("{text}\n"))?;
            return Ok(Done::exit(e.code));
        }
        other => other?,
    };
    let text = if json {
        render_json(&a)? + "\n"
    } else {
        render(&a)
    };
    out(&text)?;
    let code = a.exit_code();
    Ok(Done {
        spec_hash: Some(a.hash),
        ..Done::exit(code)
    })
}
