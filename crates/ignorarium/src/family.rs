//! Families: confirmatory studies declared together in the first one's `[family]` table, which
//! may share confirmation and reserve data. This module reads the member specs a root lists,
//! checks each with every rule, builds the lock the root's `register` signs, and finds the
//! registered family of a member.
use crate::canon::{canonical_of, sha256_hex};
use crate::cli::check::{PENDING, scheduled};
use crate::derived::Register;
use crate::diag::{Diagnostic, Severity, printable};
use crate::inputs::{InputHashes, input_path, regular_file, spec_hashes, study_root};
use crate::planlog::store::Leaf;
use crate::prior::{NoPriorUse, same_data};
use crate::rules::{self, CheckCtx, NoRecords};
use crate::spec::model::{Role, Spec};
use crate::spec::{self, RawSpec};
use crate::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The kind of the plan-log leaf that a root's `register` appends after its plan leaf.
pub const LEAF_KIND: &str = "family";

/// The label of the family leaf of root `root`. No study id holds `:`, so it names no study's
/// leaves, and no `release` frees data from it.
pub fn leaf_label(root: &str) -> String {
    format!("family:{root}")
}

/// What a root's `register` fixes about each member. A member registers later only as
/// locked here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lock {
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub id: String,
    /// The path the root lists, from the study root.
    pub spec: String,
    pub spec_hash: String,
    /// The tree hash of the payload folder the member shares with its root.
    pub payload: String,
    pub inputs: Vec<LockedInput>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedInput {
    pub path: String,
    pub role: Role,
    pub sha256: String,
}

impl Lock {
    /// `sha256:` and the hex digest of the lock's canonical JSON: the family leaf's commitment.
    pub fn hash(&self) -> String {
        let canon = canonical_of(self).expect("a lock holds only strings");
        format!("sha256:{}", sha256_hex(&canon))
    }

    /// The lock of `members`, as the root's check read them, each sharing the payload whose
    /// tree hash is `payload`. Every member must have been read and fit the typed model.
    pub fn of(members: &[Result<MemberSpec, String>], payload: &str) -> Result<Lock, String> {
        let member = |m: &Result<MemberSpec, String>| {
            let m = m.as_ref().map_err(Clone::clone)?;
            let spec = m.typed()?;
            let inputs = (spec.inputs.iter())
                .map(|i| LockedInput {
                    path: i.path.clone(),
                    role: i.role,
                    sha256: i.sha256.clone(),
                })
                .collect();
            Ok(Member {
                id: spec.study.id,
                spec: m.path.clone(),
                spec_hash: m.raw.hash.clone(),
                payload: payload.to_string(),
                inputs,
            })
        };
        let members = members.iter().map(member).collect::<Result<_, String>>()?;
        Ok(Lock { members })
    }
}

/// The registered family a member belongs to, as `find` gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// The root's study id: the family's name.
    pub root: String,
    /// The index of the root's plan leaf in `log`.
    pub root_leaf: u64,
    /// The index of the family leaf in `log`.
    pub family_leaf: u64,
    pub lock: Lock,
    /// The log the root registered in, which holds both leaves.
    pub log: String,
}

/// True when a use at leaf `leaf` of `found.log` does not count against a member of `found`:
/// the root's plan leaf, and every leaf from the family leaf on. A leaf between the two came
/// from another study, so it still counts.
pub fn exempt(found: &Found, leaf: u64) -> bool {
    leaf == found.root_leaf || leaf >= found.family_leaf
}

/// The family whose lock lists `study`, from the register record of each study (`registers`,
/// by study id) and the leaves of each log (`logs`, by log name). Each family leaf must name a
/// root that registered in its log, whose plan leaf comes before it and whose record holds a
/// lock with the leaf's commitment; and a lock that lists `study` must have its family leaf.
/// Anything else is an error, never "not a member".
pub fn find(
    registers: &[(String, Register)],
    logs: &[(&str, Vec<Leaf>)],
    study: &str,
) -> Result<Option<Found>, String> {
    let mut leaves: Vec<(&str, &str, u64)> = vec![];
    for (log, all) in logs {
        for (i, leaf) in (all.iter().enumerate()).filter(|(_, l)| l.kind == LEAF_KIND) {
            let i = i as u64;
            let shown = printable(&leaf.label);
            let root = (registers.iter())
                .find(|(id, r)| {
                    leaf.label == leaf_label(id)
                        && r.log == *log
                        && r.leaf < i
                        && leaf.commitment.is_some()
                        && r.family.as_ref().map(Lock::hash) == leaf.commitment
                })
                .map(|(id, _)| id.as_str())
                .ok_or_else(|| {
                    format!("{log} leaf {i} ({shown}) names no root whose record holds its lock")
                })?;
            if leaves.iter().any(|(l, r, _)| l == log && *r == root) {
                return Err(format!(
                    "{log} holds a second family leaf of {root}, leaf {i}"
                ));
            }
            leaves.push((log, root, i));
        }
    }
    let listing: Vec<&(String, Register)> = (registers.iter())
        .filter(|(_, r)| (r.family.iter()).any(|l| l.members.iter().any(|m| m.id == study)))
        .collect();
    let (root, reg) = match listing[..] {
        [] => return Ok(None),
        [(root, reg)] => (root, reg),
        _ => return Err(format!("more than one lock lists {study}")),
    };
    let (_, _, family_leaf) = (leaves.iter())
        .find(|(log, r, _)| *log == reg.log && r == root)
        .ok_or_else(|| {
            format!(
                "the lock of {root} lists {study}, and {} holds no family leaf for it",
                reg.log
            )
        })?;
    Ok(Some(Found {
        root: root.clone(),
        root_leaf: reg.leaf,
        family_leaf: *family_leaf,
        lock: reg.family.clone().expect("a listing register holds a lock"),
        log: reg.log.clone(),
    }))
}

/// The confirmation and reserve data of every member in `members` that was read, once each:
/// the family leaf's data hashes.
pub fn locked_data(members: &[Result<MemberSpec, String>]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    for m in members.iter().flatten() {
        for (_, h) in data(&m.raw.value, &m.input_hashes, &["confirmation", "reserve"]) {
            if !out.contains(&h) {
                out.push(h);
            }
        }
    }
    out
}

/// The plan slots that must stay free after the one `study` opens: one per member not yet
/// registered of every registered family (`unopened`), less `study`'s own when it is one, and
/// `members`, the members of the family `study` declares.
pub fn keep(unopened: &[String], study: &str, members: usize) -> u64 {
    (unopened.iter().filter(|u| *u != study).count() + members) as u64
}

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
    /// Its inputs' hashes, as `inputs::spec_hashes` gives them from the member's root.
    pub input_hashes: InputHashes,
}

impl MemberSpec {
    /// True when the member declares a family of its own. E-FAM-002 refuses it, and no other
    /// check reads it.
    pub fn nested(&self) -> bool {
        self.raw.value.get("family").is_some()
    }

    /// The member's typed spec, with defaults filled, as its check read it.
    pub fn typed(&self) -> Result<Spec, String> {
        let mut value = self.raw.value.clone();
        crate::estimate::apply_defaults(&mut value);
        let shown = printable(&self.path);
        Spec::from_value(&value).map_err(|e| format!("member {shown} does not fit the schema: {e}"))
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
            let input_hashes = spec_hashes(store, &root, &raw.value);
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
/// `hashes` is `inputs::spec_hashes` of the same spec, which lists the same inputs.
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
/// root's prior use, records, store and machine, then, as a standalone check does, the typed
/// model and the schedule when no rule failed, each told the family of root `root_id`; diagnostics as `report` gives them.
pub fn check(ctx: &mut CheckCtx, root_id: &str) -> Vec<Diagnostic> {
    let members = std::mem::take(&mut ctx.members);
    let ids = (members.iter().flatten())
        .filter_map(|m| m.raw.value.pointer("/study/id")?.as_str())
        .chain([root_id]);
    let family_ids = ids.map(str::to_string).collect();
    // The member checks borrow the root's prior use and records, so every check reads each
    // log once; they go back to the root at the end.
    let mut each = CheckCtx {
        prior: std::mem::replace(&mut ctx.prior, Box::new(NoPriorUse)),
        records: std::mem::replace(&mut ctx.records, Box::new(NoRecords)),
        machine_max_level: ctx.machine_max_level,
        family_ids,
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
        let (value, _, mut diags) = rules::check_all(&m.raw.value, rules::builtin(), &each);
        let clean = diags.iter().all(|d| d.severity != Severity::Error);
        let carried = rules::carried(&value, &each).is_some();
        if clean && scheduled(&value, &m.raw.hash, carried).is_none() {
            diags.push(rules::schema_fault());
        }
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
