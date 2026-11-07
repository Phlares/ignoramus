//! Families: confirmatory studies declared together in the first one's `[family]` table, which
//! may share confirmation and reserve data. This module reads the member specs a root lists
//! and checks each with every rule.
use crate::cli::check::{PENDING, input_hashes, study_root};
use crate::diag::{Diagnostic, printable};
use crate::inputs::{input_path, regular_file};
use crate::prior::{NoPriorUse, same_data};
use crate::rules::{self, CheckCtx, NoRecords};
use crate::runner::InputHashes;
use crate::spec::{self, RawSpec};
use crate::store::Store;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The code a member's `PENDING` diagnostic takes in its root's check.
pub const WAITS: &str = "W-FAM-001";

/// A member spec a root lists, as the root's check read it.
#[derive(Debug)]
pub struct MemberSpec {
    /// Its place in `family.members`.
    pub index: usize,
    /// The path the root lists, from the study root.
    pub path: String,
    pub raw: RawSpec,
    /// The member's own study root, found from its spec's folder.
    pub root: PathBuf,
    /// Its inputs' hashes, as `check::input_hashes` gives them from the member's root.
    pub input_hashes: InputHashes,
}

impl MemberSpec {
    /// True when the member declares a family of its own. E-FAM-002 refuses it, and no other
    /// check reads it.
    pub fn nested(&self) -> bool {
        self.raw.value.get("family").is_some()
    }
}

/// One entry per element of `spec`'s `family.members`, in order: the member read from under
/// `root`, or why it cannot be read. A path that is not a string or could leave `root` is
/// the generic rules' to report.
pub fn load(root: &Path, spec: &Value, store: &Store) -> Vec<Result<MemberSpec, String>> {
    let list = spec.pointer("/family/members").and_then(Value::as_array);
    (list.map_or(&[][..], Vec::as_slice).iter().enumerate())
        .map(|(index, entry)| {
            let path = entry.as_str().ok_or("The member is not a path.")?;
            input_path(root, path).ok_or("The path could leave the study root.")?;
            let file = regular_file(root, path).ok_or("No spec file is at this path.")?;
            let unread = |e: String| printable(&format!("The spec cannot be read: {e}."));
            let bytes = std::fs::read(&file).map_err(|e| unread(e.to_string()))?;
            let raw = spec::parse(&bytes).map_err(|e| unread(e.to_string()))?;
            let root = study_root(&file);
            let input_hashes = input_hashes(store, &root, &raw.value);
            Ok(MemberSpec {
                index,
                path: path.to_string(),
                raw,
                root,
                input_hashes,
            })
        })
        .collect()
}

/// Each input of `spec` that has a path, as its spec path, its table and its data's hashes;
/// `hashes` is `check::input_hashes` of the same spec, which lists the same inputs.
pub fn inputs<'a>(
    spec: &'a Value,
    hashes: &'a InputHashes,
) -> impl Iterator<Item = (String, &'a Value, Option<&'a (String, String)>)> {
    let list = spec["inputs"].as_array().map_or(&[][..], Vec::as_slice);
    (list.iter().enumerate())
        .filter(|(_, i)| i["path"].as_str().is_some())
        .zip(hashes)
        .map(|((k, i), (_, h))| (format!("inputs[{k}]"), i, h.as_ref()))
}

/// The data of each input of `spec` with one of `roles` whose bytes were read, as its spec
/// path and hashes.
pub fn data(spec: &Value, hashes: &InputHashes, roles: &[&str]) -> Vec<(String, (String, String))> {
    inputs(spec, hashes)
        .filter(|(_, i, _)| i["role"].as_str().is_some_and(|r| roles.contains(&r)))
        .filter_map(|(at, _, h)| Some((at, h?.clone())))
        .collect()
}

/// Each entry of `a` that holds the data of an entry of `b`, by exact or normalized hash, with
/// the first such entry of `b`, as their two names.
pub fn shared<'a>(
    a: &'a [(String, (String, String))],
    b: &'a [(String, (String, String))],
) -> Vec<(&'a str, &'a str)> {
    (a.iter())
        .filter_map(|(x, hx)| {
            let (y, _) = b.iter().find(|(_, hy)| same_data(hx, hy))?;
            Some((x.as_str(), y.as_str()))
        })
        .collect()
}

/// Every rule on each member in `ctx.members` that declares no family of its own, with the
/// root's prior use, records, store and machine, as `report` gives the diagnostics.
pub fn check(ctx: &mut CheckCtx) -> Vec<Diagnostic> {
    let members = std::mem::take(&mut ctx.members);
    // The member checks borrow the root's prior use and records, so every check reads each
    // log once; they go back to the root at the end.
    let mut each = CheckCtx {
        prior: std::mem::replace(&mut ctx.prior, Box::new(NoPriorUse)),
        records: std::mem::replace(&mut ctx.records, Box::new(NoRecords)),
        machine_max_level: ctx.machine_max_level,
        store: Store {
            root: ctx.store.root.clone(),
        },
        ..CheckCtx::empty(&ctx.root)
    };
    let mut out = vec![];
    for m in members.iter().flatten().filter(|m| !m.nested()) {
        each.root.clone_from(&m.root);
        each.spec_text.clone_from(&m.raw.text);
        each.spec_hash = spec::hash_bytes(&m.raw.hash).unwrap_or_default();
        each.gates = Default::default();
        each.payload = Default::default();
        let (_, _, diags) = rules::check_all(&m.raw.value, rules::builtin(), &each);
        out.extend(report(m, diags));
    }
    ctx.prior = each.prior;
    ctx.records = each.records;
    ctx.members = members;
    out
}

/// Member `m`'s diagnostics as its root reports them: at `family.members[<i>]:<path>`, the
/// problem led by the member's own field, and a value that waits on an unfinished study as
/// `WAITS`, so it does not hold the root's check at exit 3.
pub fn report(m: &MemberSpec, diags: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let at = format!("family.members[{}]:{}", m.index, printable(&m.path));
    let pending = rules::raised(PENDING, String::new(), None).problem;
    (diags.into_iter())
        .map(|d| {
            if d.code == PENDING {
                let why = d.problem.strip_prefix(&pending).unwrap_or(&d.problem);
                let detail = format!("At {}: {}", d.path, why.trim_start());
                return rules::raised(WAITS, at.clone(), Some(&detail));
            }
            Diagnostic {
                path: at.clone(),
                problem: format!("At {}: {}", d.path, d.problem),
                ..d
            }
        })
        .collect()
}
