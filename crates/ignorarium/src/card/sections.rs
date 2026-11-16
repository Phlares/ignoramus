//! The card's sections (design 2.1), each built from the verified log and the registered plan.
//! Files of the study folder that no key signs enter only by path and SHA-256.
use super::Interval;
use super::recipe::{children, plain};
use super::sidecar::DataList;
use crate::analysis::{IntervalResult, claim_strata};
use crate::canon::sha256_file;
use crate::derived::{
    Analysis, DONE, DoneRecord, End, Estimate, GroupClip, Instrument, Register, Resolved,
    Selection, Stamp, Trigger, Verdict,
};
use crate::files::optional;
use crate::records::{Log, Record};
use crate::rules::{Curve, CurvePoint};
use crate::runner::Tally;
use crate::spec::model::{Budget, Claim, Mode, Paired, SelectionRule, Spec, StatisticSpec};
use crate::telemetry::Totals;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// What a renderer prints for a field no record holds.
pub const NOT_IN_RECORDS: &str = "not in the records";
/// What a renderer prints for a field that has no value in this study.
pub const NOT_APPLICABLE: &str = "not applicable";
/// The fixed line under the list of runs.
pub const UNRECORDED: &str = "Executions recorded nowhere cannot be listed.";
/// How a row's seed derives from the plan hash (`seed::row_hex`).
pub const ROW_SEED: &str = "SHA-256(plan hash bytes || \"row/<index>\"); a recheck copy keeps the seed of the row it repeats";
/// Only the level 2 backend runs an agent beside the payload.
const AGENT_LEVEL: u8 = 2;
/// The claim label line until the box computes labels (audit G1).
pub const LABEL_NOT_COMPUTED: &str = "label not computed";
/// The results note of a quantile claim (ruling R8).
pub const NO_QUANTILE_INTERVAL: &str = "the interval is not built for quantile claims";
/// The files of the study folder the card lists by name, in order. Every file under each of
/// `TREES` follows them.
pub const LISTED: [&str; 4] = [
    "gaps.md",
    "decisions.md",
    crate::deviations::FILE,
    super::recipe::CITATION_FILE,
];
/// The trees of the study folder the card lists file by file, in order: the preregistered
/// documents, among them the statement, and the descriptive outputs, files no registered test
/// reads.
pub const TREES: [&str; 2] = ["prereg", "descriptive"];
/// Why an outside file is missing when no `--study-folder` is given.
pub const NO_STUDY_FOLDER: &str = "no study folder given";
/// Why a link in the study folder is missing from the list.
pub const LINK_NOT_FOLLOWED: &str = "link not followed";
/// The run log, which no key signs.
pub const EVENTS: &str = "events.jsonl";

/// A file of the study folder, by its path in the folder and its SHA-256. Never parsed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outside {
    pub path: String,
    pub sha256: String,
}

impl Outside {
    /// File `name` of the study folder `dir`; `None` when there is none.
    pub fn of(dir: &Path, name: &str) -> Result<Option<Outside>, String> {
        let hash = optional(sha256_file(&dir.join(name))).map_err(|e| format!("{name}: {e}"))?;
        Ok(hash.map(|sha256| Outside {
            path: name.into(),
            sha256,
        }))
    }
}

/// A card value: from the records, missing from them, or with no value in this study.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Field<T> {
    Recorded {
        value: T,
    },
    /// `link` is the study-folder file that holds the value, when there is one.
    Missing {
        link: Option<Outside>,
    },
    NotApplicable,
}

/// The default of a field a card built before it existed does not hold.
pub(crate) fn missing<T>() -> Field<T> {
    Field::Missing { link: None }
}

impl<T> Field<T> {
    pub(crate) fn of(value: Option<T>, link: Option<&Outside>) -> Field<T> {
        match value {
            Some(value) => Field::Recorded { value },
            None => Field::Missing {
                link: link.cloned(),
            },
        }
    }

    /// The value as `show` prints it, `NOT_IN_RECORDS` or `NOT_APPLICABLE`.
    pub fn shown(&self, show: impl FnOnce(&T) -> String) -> String {
        match self {
            Field::Recorded { value } => show(value),
            Field::Missing { .. } => NOT_IN_RECORDS.into(),
            Field::NotApplicable => NOT_APPLICABLE.into(),
        }
    }
}

/// An outside file; `sha256` is `None` when the study folder does not hold it, and `note`
/// says why when it could not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Listed {
    pub path: String,
    pub sha256: Option<String>,
    pub note: Option<String>,
}

/// Where the outside files are pinned: the study folder's path inside its git repo, and the
/// full hash of the repo's HEAD.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub path: String,
    pub commit: String,
}

/// Every outside file, and the path of the author slots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutsideFiles {
    pub files: Vec<Listed>,
    pub readings: String,
}

impl OutsideFiles {
    /// Whether the study folder holds file `name` as a regular file.
    pub fn holds(&self, name: &str) -> bool {
        (self.files.iter()).any(|f| f.path == name && f.sha256.is_some())
    }
}

/// The paths under tree `root` of `dir` that are files or links, sorted; none when the tree is
/// absent. A link is not entered. Refuses what the manifest walk refuses.
fn tree(dir: &Path, root: &str) -> Result<Vec<String>, String> {
    let mut found = vec![];
    let mut pending = vec![(dir.join(root), root.to_string())];
    while let Some((full, rel)) = pending.pop() {
        plain(&rel, "the path")?;
        let kind = match std::fs::symlink_metadata(&full) {
            Ok(meta) => meta.file_type(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && rel == root => continue,
            Err(e) => return Err(format!("{rel}: {e}")),
        };
        if kind.is_dir() {
            for (name, _) in children(&full, &rel)? {
                pending.push((full.join(&name), format!("{rel}/{name}")));
            }
        } else if kind.is_file() || kind.is_symlink() {
            found.push(rel);
        } else {
            return Err(format!("{rel}: not a file, directory or link"));
        }
    }
    found.sort();
    Ok(found)
}

/// File `name` of `dir` by hash. A link is not followed.
fn listed(dir: &Path, name: &str) -> Result<Listed, String> {
    let listed = |sha256, note: Option<&str>| Listed {
        path: name.into(),
        sha256,
        note: note.map(String::from),
    };
    let link = (optional(std::fs::symlink_metadata(dir.join(name))))
        .map_err(|e| format!("{name}: {e}"))?
        .is_some_and(|m| m.file_type().is_symlink());
    if link {
        return Ok(listed(None, Some(LINK_NOT_FOLLOWED)));
    }
    Ok(listed(Outside::of(dir, name)?.map(|o| o.sha256), None))
}

/// Runs git in `dir`; returns its stdout without the final line break. Each line git printed
/// passes the control-character check.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(["--literal-pathspecs", "-c", "core.fsmonitor=false", "-C"])
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "git not found; --study-folder needs git".to_string(),
            _ => format!("cannot run git: {e}"),
        })?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        return Err(format!("git {}: {}", args[0], why.trim()));
    }
    let text = String::from_utf8(out.stdout).map_err(|_| format!("git {}: not UTF-8", args[0]))?;
    let text = text.strip_suffix('\n').unwrap_or(&text);
    (text.lines()).try_for_each(|line| plain(line, "the output of git").map(drop))?;
    Ok(text.to_string())
}

/// The outside files of the study folder `dir`, pinned by its git commit. With no folder every
/// file is missing. Refuses a folder that is not in a git work tree, and a listed file that is
/// untracked, ignored or changed since the commit.
pub fn outside(dir: Option<&Path>) -> Result<(OutsideFiles, Option<Source>), String> {
    let readings = super::readings::FILE.to_string();
    let Some(dir) = dir else {
        let files = LISTED.map(|name| Listed {
            path: name.into(),
            sha256: None,
            note: Some(NO_STUDY_FOLDER.into()),
        });
        return Ok((
            OutsideFiles {
                files: files.to_vec(),
                readings,
            },
            None,
        ));
    };
    if !dir.is_dir() {
        return Err(format!("{}: not a folder", dir.display()));
    }
    let not_inside = |why: &str| format!("{}: not inside a git work tree{why}", dir.display());
    let inside = git(dir, &["rev-parse", "--is-inside-work-tree"])
        .map_err(|e| not_inside(&format!(" ({e})")))?;
    if inside != "true" {
        return Err(not_inside(""));
    }
    let trees = (TREES.iter().map(|t| tree(dir, t))).collect::<Result<Vec<_>, _>>()?;
    let names = LISTED.map(String::from).into_iter().chain(trees.concat());
    let files = names
        .map(|name| listed(dir, &name))
        .collect::<Result<Vec<_>, _>>()?;
    let held: Vec<&str> = (files.iter())
        .filter(|f| f.sha256.is_some())
        .map(|f| f.path.as_str())
        .collect();
    let mut status = vec!["status", "--porcelain", "--ignored", "--"];
    status.extend(&held);
    let changed = git(dir, &status)?;
    if !changed.is_empty() {
        return Err(format!(
            "{}: a listed file is untracked or has uncommitted changes:\n{changed}",
            dir.display()
        ));
    }
    let source = Source {
        path: git(dir, &["rev-parse", "--show-prefix"])?,
        commit: git(dir, &["rev-parse", "HEAD"])?,
    };
    Ok((OutsideFiles { files, readings }, Some(source)))
}

/// The first estimate of each hypothesis, contrast and stratum in `after`, as `report`
/// counts them.
fn estimates(after: &[Record]) -> Vec<Estimate> {
    let mut seen = BTreeSet::new();
    (after.iter().filter_map(Estimate::of))
        .filter(|e| seen.insert(e.identity()))
        .collect()
}

/// The first verdict of each hypothesis in `records`; a later one is a repeat.
pub fn first_verdicts(records: &[Record]) -> Vec<Verdict> {
    let mut seen = BTreeSet::new();
    (records.iter().filter_map(Verdict::of))
        .filter(|v| seen.insert(v.hypothesis.clone()))
        .collect()
}

fn rows(log: &Log) -> impl Iterator<Item = &Record> {
    log.records.iter().filter(|r| r.kind == "row")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub mode: Mode,
    pub blind: bool,
    pub spec_hash: String,
    /// The time on the register record, from the box's clock.
    pub registered_at: String,
    /// Each outside time stamp of the log, in record order. Empty in a card written before
    /// stamps existed.
    #[serde(default)]
    pub stamps: Vec<StampLine>,
    /// How the study ended; missing for a log with no `done` record. A stopped study has no
    /// card, so the end here is `rows` or `resize-stop`.
    #[serde(default = "missing")]
    pub ended: Field<Ended>,
    /// The plan's `question.population`: the frame every claim covers. Missing when the plan
    /// leaves it empty.
    #[serde(default = "missing")]
    pub population: Field<String>,
}

/// The `done` record: how the study ended, when, and the record's seq and hash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ended {
    pub end: End,
    pub ended_at: String,
    pub seq: u64,
    pub hash: String,
}

/// One `stamp` record. `head_seq` is the last record the stamp covers, `None` when the file
/// held no head line: the registration alone. `results` is true when it covers the `done`
/// record. `signatures` is true when the file held the register signature line and, with a
/// head, the head signature line. `flag` is set when the stamp is dated before a record it
/// covers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StampLine {
    pub seq: u64,
    pub stamped_at: String,
    pub authority: Option<String>,
    pub head_seq: Option<u64>,
    pub results: bool,
    pub key: bool,
    pub signatures: bool,
    pub file_sha256: String,
    pub reply_sha256: String,
    pub flag: Option<String>,
}

/// The flag for `s` when it is dated before the newest record it covers: the head record,
/// else the register record. `ts` and `stamped_at` share one UTC format, so comparing the
/// strings compares the times.
pub fn stamp_flag(all: &[Record], s: &Stamp) -> Option<String> {
    let head = s.lines.head.as_ref().map(|h| h.seq);
    let covered = head
        .and_then(|h| all.iter().find(|r| r.seq == h))
        .or(all.first())?;
    (s.stamped_at < covered.ts)
        .then(|| format!("dated before record {}, which it covers", covered.seq))
}

/// The `stamp` records of `all`, in order.
pub fn stamps(all: &[Record]) -> Vec<StampLine> {
    let done = all.iter().find(|r| r.kind == DONE).map(|r| r.seq);
    (all.iter())
        .filter_map(|r| Some((r, Stamp::of(r)?)))
        .map(|(r, s)| {
            let head = s.lines.head.as_ref().map(|h| h.seq);
            StampLine {
                seq: r.seq,
                flag: stamp_flag(all, &s),
                results: matches!((head, done), (Some(h), Some(d)) if h >= d),
                key: s.lines.key,
                signatures: s.lines.register_signature
                    && (head.is_none() || s.lines.head_signature),
                head_seq: head,
                stamped_at: s.stamped_at,
                authority: s.authority,
                file_sha256: s.file_sha256,
                reply_sha256: s.reply_sha256,
            }
        })
        .collect()
}

/// The first `done` record of `all`.
pub fn ended(all: &[Record]) -> Field<Ended> {
    let done = all.iter().find_map(|r| Some((r, DoneRecord::of(r)?)));
    Field::of(
        done.map(|(r, d)| Ended {
            end: d.end,
            ended_at: d.ended_at,
            seq: r.seq,
            hash: crate::records::hash(r),
        }),
        None,
    )
}

/// The header of a study whose register record `own` was written at `registered_at`; `all` is
/// its verified log.
pub fn header(own: &Register, registered_at: &str, spec: &Spec, all: &[Record]) -> Header {
    Header {
        mode: spec.study.mode,
        blind: spec.access.blind,
        spec_hash: own.spec_hash.clone(),
        registered_at: registered_at.into(),
        stamps: stamps(all),
        ended: ended(all),
        population: Field::of(
            Some(spec.question.population.clone()).filter(|p| !p.is_empty()),
            None,
        ),
    }
}

/// One registered hypothesis: its claim, and the e-value its test must reach.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimLine {
    pub hypothesis: String,
    pub statement: String,
    pub outcome: String,
    /// Empty: every confirmation cluster as one group.
    pub strata: Vec<String>,
    pub claim: Option<Claim>,
    pub statistic: Option<StatisticSpec>,
    pub level: Option<f64>,
    /// 1 / `level`: the e-value the test must reach in each stratum.
    pub pass_e_value: Option<f64>,
}

pub fn claims(after: &[Record], spec: &Spec) -> Vec<ClaimLine> {
    let levels: BTreeMap<String, Option<f64>> = (first_verdicts(after).into_iter())
        .map(|v| (v.hypothesis, v.level))
        .collect();
    (spec.question.hypotheses.iter())
        .map(|h| {
            let level = levels.get(&h.id).copied().flatten();
            ClaimLine {
                hypothesis: h.id.clone(),
                statement: h.statement.clone(),
                outcome: h.outcome.clone(),
                strata: h.strata.clone(),
                claim: h.claim,
                statistic: h.statistic,
                level,
                pass_e_value: level.map(|l| 1.0 / l),
            }
        })
        .collect()
}

/// The candidate a selection stage picked, from its signed `selection` record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pick {
    pub selected: String,
    pub rule: SelectionRule,
    pub metric: String,
    /// In the order the stage lists them.
    pub candidates: Vec<String>,
    pub complete_units: u64,
}

/// `None` when the log holds no `selection` record: the study had no selection stage.
pub fn pick(log: &Log) -> Option<Pick> {
    (log.records.iter().find_map(Selection::of)).map(|s| Pick {
        selected: s.selected,
        rule: s.rule,
        metric: s.metric,
        candidates: s.candidates.into_iter().map(|c| c.id).collect(),
        complete_units: s.complete_units,
    })
}

/// A verdict as the card words it: `pass` is shown, `fail` is not shown, `none` makes no claim.
pub fn display_word(verdict: &str) -> &str {
    match verdict {
        "pass" => "shown",
        "fail" => "not shown",
        "none" => "no claim",
        other => other,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerdictLine {
    pub hypothesis: String,
    pub display: String,
    pub label: String,
}

pub fn verdicts(after: &[Record]) -> Vec<VerdictLine> {
    (first_verdicts(after).into_iter())
        .map(|v| VerdictLine {
            display: display_word(&v.verdict).into(),
            hypothesis: v.hypothesis,
            label: LABEL_NOT_COMPUTED.into(),
        })
        .collect()
}

/// One estimate record of a hypothesis in one group (`""` pools every cluster).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultRow {
    pub hypothesis: String,
    pub stratum: String,
    pub outcome: String,
    pub estimate: f64,
    pub clusters: u64,
    pub units: u64,
    /// exp(`estimate`) - 1, for an outcome paired as a log ratio.
    pub percent_change: Option<f64>,
    /// The least e-value of the claim's bounds in this group; `None` where no bound is held.
    pub e_value: Option<f64>,
    /// The group's intervals, never registered. The card holds them here only.
    pub intervals: Vec<Interval>,
    pub note: Option<String>,
    /// The outcome is registered.
    pub registered: bool,
    /// No registered claim is held in this group: the row carries no claim.
    #[serde(default)]
    pub descriptive: bool,
}

pub fn results(after: &[Record], spec: &Spec, intervals: &[IntervalResult]) -> Vec<ResultRow> {
    let verdicts = first_verdicts(after);
    (estimates(after).into_iter())
        .map(|e| {
            let h = (spec.question.hypotheses.iter()).find(|h| h.id == e.hypothesis);
            let outcome = spec.outcomes.iter().find(|o| o.id == e.outcome);
            let log_ratio = outcome.is_some_and(|o| o.paired == Paired::LogRatio);
            let claimed = h.is_some_and(|h| {
                h.claim.is_some() && claim_strata(h).contains(&e.stratum.as_str())
            });
            let quantile = h.is_some_and(|h| {
                h.claim.is_some() && matches!(h.statistic, Some(StatisticSpec::Quantile(_)))
            });
            let e_value = (verdicts.iter().filter(|v| v.hypothesis == e.hypothesis))
                .flat_map(|v| &v.bounds)
                .filter(|b| b.stratum == e.stratum)
                .filter_map(|b| b.value)
                .reduce(f64::min);
            let intervals = (intervals.iter())
                .filter(|i| i.hypothesis == e.hypothesis && i.stratum == e.stratum)
                .map(Interval::from)
                .collect();
            ResultRow {
                percent_change: log_ratio.then(|| e.mean.exp() - 1.0),
                e_value,
                intervals,
                note: quantile.then(|| NO_QUANTILE_INTERVAL.into()),
                registered: outcome.is_some_and(|o| o.registered),
                descriptive: !claimed,
                hypothesis: e.hypothesis,
                stratum: e.stratum,
                outcome: e.outcome,
                estimate: e.mean,
                clusters: e.clusters,
                units: e.units,
            }
        })
        .collect()
}

/// The power gate's prediction for one hypothesis, as the register record holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sensitivity {
    pub hypothesis: String,
    pub power: f64,
    pub target: f64,
    pub power_at: f64,
    pub curve: Vec<Curve>,
    pub placed: Vec<Placed>,
}

/// Where one group's estimate falls on the curve toward `margin`: the nearest point at or
/// below it and at or above it; `None` past an end of the curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    pub margin: f64,
    pub stratum: String,
    pub estimate: f64,
    pub lower: Option<CurvePoint>,
    pub upper: Option<CurvePoint>,
}

fn place(curve: &Curve, stratum: &str, estimate: f64) -> Placed {
    let points = curve.points.iter();
    let by_mean = |a: &&CurvePoint, b: &&CurvePoint| a.mean.total_cmp(&b.mean);
    Placed {
        margin: curve.margin,
        stratum: stratum.into(),
        estimate,
        lower: (points
            .clone()
            .filter(|p| p.mean <= estimate)
            .max_by(by_mean))
        .cloned(),
        upper: (points.filter(|p| p.mean >= estimate).min_by(by_mean)).cloned(),
    }
}

/// The gate's curves from the register record `own`, with the estimates in `after`.
pub fn sensitivity(own: &Register, after: &[Record], spec: &Spec) -> Vec<Sensitivity> {
    let estimates = estimates(after);
    (own.gate.iter())
        .map(|g| {
            let strata = (spec.question.hypotheses.iter())
                .find(|h| h.id == g.id)
                .map(claim_strata)
                .unwrap_or_default();
            let held = (estimates.iter())
                .filter(|e| e.hypothesis == g.id && strata.contains(&e.stratum.as_str()));
            let placed = (g.curve.iter())
                .flat_map(|c| held.clone().map(|e| place(c, &e.stratum, e.mean)))
                .collect();
            Sensitivity {
                hypothesis: g.id.clone(),
                power: g.power,
                target: g.target,
                power_at: g.power_at,
                curve: g.curve.clone(),
                placed,
            }
        })
        .collect()
}

/// The machine each row ran in (disclosure part 4). A value the row records do not hold
/// links the run log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stack {
    /// Each value the rows name, sorted.
    pub isolation: Field<Vec<u8>>,
    pub launcher: Field<Vec<String>>,
    pub agent_sha256: Field<Vec<String>>,
    /// Level 2 rows: each vCPU count and machine memory the rows name, sorted.
    pub vcpus: Field<Vec<u32>>,
    #[serde(default = "missing")]
    pub vm_mem_mib: Field<Vec<u32>>,
    /// The plan's `row_memory_mib`.
    pub memory_cap_mib: Field<u32>,
    /// Per key above, the row records whose value does not parse; keys with none are left out.
    #[serde(default)]
    pub unreadable: BTreeMap<String, u64>,
}

/// The distinct values of `key` over the row records of `log`, `None` when no row holds one,
/// and the count of rows whose value does not parse, added to `unreadable`.
fn from_rows<T: Ord + DeserializeOwned>(
    log: &Log,
    key: &str,
    unreadable: &mut BTreeMap<String, u64>,
) -> Option<Vec<T>> {
    let mut values = BTreeSet::new();
    let mut bad = 0;
    for v in rows(log).filter_map(|r| r.body.get(key)) {
        match serde_json::from_value(v.clone()) {
            Ok(v) => {
                values.insert(v);
            }
            Err(_) => bad += 1,
        }
    }
    if bad > 0 {
        unreadable.insert(key.into(), bad);
    }
    (!values.is_empty()).then(|| values.into_iter().collect())
}

/// A value only level 2 rows hold: not applicable when every row ran below level 2.
fn in_machine<T: Ord + DeserializeOwned>(
    log: &Log,
    key: &str,
    agentless: bool,
    events: Option<&Outside>,
    unreadable: &mut BTreeMap<String, u64>,
) -> Field<Vec<T>> {
    match from_rows(log, key, unreadable) {
        None if agentless => Field::NotApplicable,
        found => Field::of(found, events),
    }
}

pub fn stack(log: &Log, spec: &Spec, events: Option<&Outside>) -> Stack {
    let mut unreadable = BTreeMap::new();
    let levels: Option<Vec<u8>> = from_rows(log, "isolation", &mut unreadable);
    let agentless = (levels.as_ref()).is_some_and(|l| l.iter().all(|l| *l < AGENT_LEVEL));
    let agent_sha256 = in_machine(log, "agent_sha256", agentless, events, &mut unreadable);
    let vcpus = in_machine(log, "vcpus", agentless, events, &mut unreadable);
    let vm_mem_mib = in_machine(log, "vm_mem_mib", agentless, events, &mut unreadable);
    Stack {
        isolation: Field::of(levels, events),
        launcher: Field::of(from_rows(log, "launcher", &mut unreadable), events),
        agent_sha256,
        vcpus,
        vm_mem_mib,
        memory_cap_mib: Field::of(spec.design.row_memory_mib, None),
        unreadable,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Setup {
    /// The signed input list: the count and hash of its lines, which `data.sha256` holds.
    pub data: DataList,
    /// Data grades are not defined yet (audit C6).
    pub data_grade: Field<String>,
    pub instrument: Instrument,
    /// Every value the box filled in for the plan.
    pub defaults: Value,
    /// Every value taken from another study.
    pub resolved: Vec<Resolved>,
    pub seeds: Seeds,
    pub stack: Stack,
}

/// The study's seed and how its rows' seeds derive from it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Seeds {
    /// The plan hash of the register record: every draw derives from it.
    pub root: Field<String>,
    pub rule: String,
    /// Row records that hold a seed, and those whose seed `rule` gives from `root`.
    pub rows: u64,
    pub derived: u64,
}

fn seeds(log: &Log, own: &Register) -> Seeds {
    let bytes = crate::spec::hash_bytes(&own.spec_hash);
    let seeded: Vec<&Record> = rows(log).filter(|r| r.body.get("seed").is_some()).collect();
    let derived = (seeded.iter())
        .filter(|r| {
            let index = r.body["recheck_of"].as_u64().or(r.row);
            let want = bytes.zip(index).map(|(h, i)| crate::seed::row_hex(&h, i));
            want.is_some_and(|w| r.body["seed"] == w.as_str())
        })
        .count();
    Seeds {
        root: Field::of(Some(own.spec_hash.clone()), None),
        rule: ROW_SEED.into(),
        rows: seeded.len() as u64,
        derived: derived as u64,
    }
}

pub fn setup(
    log: &Log,
    own: &Register,
    spec: &Spec,
    events: Option<&Outside>,
    data: DataList,
) -> Setup {
    Setup {
        data,
        data_grade: Field::of(None, None),
        instrument: (log.records.iter().find_map(Analysis::of))
            .map(|a| a.instrument)
            .unwrap_or_default(),
        defaults: own.defaults.clone(),
        seeds: seeds(log, own),
        resolved: own.resolved.clone(),
        stack: stack(log, spec, events),
    }
}

/// A row record whose exit is not `ok`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailedRow {
    pub row: Option<u64>,
    pub stage: String,
    pub exit: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Surprises {
    pub failed: Vec<FailedRow>,
    /// Attempts past the first, over every row.
    pub retried: u64,
    pub clipped: Vec<GroupClip>,
    pub triggers: Vec<Trigger>,
    /// Each box version an analysis record names; more than one is drift.
    pub box_versions: Vec<String>,
}

fn text(r: &Record, key: &str) -> Option<String> {
    r.body.get(key).and_then(Value::as_str).map(str::to_string)
}

pub fn surprises(log: &Log) -> Surprises {
    let analyses: Vec<Analysis> = log.records.iter().filter_map(Analysis::of).collect();
    Surprises {
        failed: (rows(log).filter(|r| r.body["exit"] != "ok"))
            .map(|r| FailedRow {
                row: r.row,
                stage: text(r, "stage").unwrap_or_default(),
                exit: text(r, "exit").unwrap_or_default(),
                reason: text(r, "reason"),
            })
            .collect(),
        retried: Tally::of(&log.records).retried,
        clipped: (analyses.first()).map_or_else(Vec::new, |a| a.clipped.clone()),
        triggers: log.records.iter().filter_map(Trigger::of).collect(),
        box_versions: (analyses.into_iter().map(|a| a.box_version))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    }
}

/// Spend in the `row` and `spend` records, the box's CPU included, against the plan's budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub cpu_seconds: f64,
    pub wall_seconds: f64,
    /// Completed rows.
    pub rows: u64,
    /// Attempts whose CPU count the backend could not read; each was charged the row estimate.
    pub unknown_cpu: u64,
    /// The machine's rate is in its config; no record holds it.
    pub usd: Field<f64>,
    pub budget: Budget,
    pub telemetry: Totals,
}

pub fn cost(log: &Log, spec: &Spec) -> Cost {
    let t = Tally::of(&log.records);
    Cost {
        cpu_seconds: t.cpu,
        wall_seconds: t.wall,
        rows: t.rows,
        unknown_cpu: t.unknown_cpu,
        usd: Field::of(None, None),
        budget: spec.scope.budget.clone(),
        telemetry: Totals::of(rows(log).map(|r| &r.body)),
    }
}

/// The `ok` row records of one stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunGroup {
    pub stage: String,
    pub rows: u64,
}

/// An invalid attempt: a `row` record whose exit is not `ok`, or a `spend` record, which holds
/// an attempt no row record holds. `cpu_seconds` is the attempt's own; the box's is left out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    pub row: Option<u64>,
    pub kind: String,
    pub exit: Option<String>,
    pub reason: Option<String>,
    pub cpu_seconds: Option<f64>,
}

/// Every execution the log records, invalid ones included (`UNRECORDED` for the rest).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Runs {
    pub ok: Vec<RunGroup>,
    /// In log order.
    pub attempts: Vec<Attempt>,
    /// The signed `session.start` records; `Missing` when the log holds none (a study run
    /// before they existed), as the unsigned events then hold the sessions.
    pub sessions: Field<u64>,
    /// Of those, the starts with no end record: a process that died. `Some` only when
    /// session records exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions_without_end: Option<u64>,
    /// Session records whose body does not parse; `Some` only when session records exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions_unreadable: Option<u64>,
    /// The reason in the study's `stop` record.
    pub stopped: Option<String>,
    /// The rows by their self-test; `Missing` on a card built before it.
    #[serde(default = "missing")]
    pub selftest: Field<SelfTestTally>,
}

/// The row records by the self-test of their last attempt. The four counts add up to the
/// row records.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SelfTestTally {
    pub passed: u64,
    pub failed_rows: u64,
    /// Per check name, the rows where it failed.
    pub failed: BTreeMap<String, u64>,
    /// Rows at level 0, which runs no sandbox.
    pub level0: u64,
    /// Rows with no `selftest` key: written before it existed, or the self-test said nothing.
    pub not_recorded: u64,
}

impl SelfTestTally {
    fn of(log: &Log) -> SelfTestTally {
        let mut t = SelfTestTally::default();
        for r in rows(log) {
            let names = r.body["selftest"]["failed"].as_array();
            match names {
                _ if r.body["isolation"] == 0 => t.level0 += 1,
                None => t.not_recorded += 1,
                Some(n) if n.is_empty() => t.passed += 1,
                Some(n) => {
                    t.failed_rows += 1;
                    for name in n.iter().filter_map(Value::as_str) {
                        *t.failed.entry(name.into()).or_default() += 1;
                    }
                }
            }
        }
        t
    }
}

pub fn runs(log: &Log, events: Option<&Outside>) -> Runs {
    let mut ok: BTreeMap<String, u64> = BTreeMap::new();
    let mut attempts = vec![];
    for r in &log.records {
        let row = r.kind == "row";
        if row && r.body["exit"] == "ok" {
            *ok.entry(text(r, "stage").unwrap_or_default()).or_default() += 1;
        } else if row || r.kind == "spend" {
            attempts.push(Attempt {
                row: r.row,
                kind: r.kind.clone(),
                exit: text(r, "exit"),
                reason: text(r, "reason"),
                cpu_seconds: r.body["cpu_seconds"].as_f64(),
            });
        }
    }
    let held = crate::derived::sessions(&log.records);
    let some = |n| (held.started > 0).then_some(n);
    Runs {
        ok: (ok.into_iter())
            .map(|(stage, rows)| RunGroup { stage, rows })
            .collect(),
        attempts,
        sessions: Field::of(some(held.started), events),
        sessions_without_end: some(held.without_end),
        sessions_unreadable: some(held.unreadable),
        stopped: crate::cli::stopped_why(&log.records),
        selftest: Field::Recorded {
            value: SelfTestTally::of(log),
        },
    }
}
