//! The study card: everything `report` says about a finished study, as one structure.
//! Each output format renders this structure; none reads the log on its own.
pub mod readings;
pub mod recipe;
pub mod render;
pub mod sections;
pub mod sidecar;

use crate::analysis::{IntervalResult, Source};
use crate::cli::check::HomeRecords;
use crate::cli::status::{relabelled, shown_labels};
use crate::cli::{CliError, not_registered, stopped_why};
use crate::control::{DeviationRow as Recorded, Timing};
use crate::derived::{
    Analysis, ChildEstimate, Estimate, OPENED, PinDeviated, Register, SpawnStop, StudyState,
    Verdict,
};
use crate::deviations::Deviations;
use crate::home::Home;
use crate::records::{Log, Record};
use crate::rules::BoxRecords;
use recipe::{Citation, Recipe};
use sections::{
    ClaimLine, Cost, Field, Header, Outside, OutsideFiles, ResultRow, Runs, Sensitivity, Setup,
    Surprises, VerdictLine,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sidecar::{Observations, Sidecars};
use std::collections::BTreeSet;
use std::path::Path;

/// The version of the card's JSON form.
pub const CARD_VERSION: u32 = 2;

/// The `opened` record that marks the card as read. `build` leaves it zeroed; the caller that
/// appends the record fills it in.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Opened {
    pub seq: u64,
    pub ts: String,
}

/// One hypothesis and its verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HypothesisCard {
    pub hypothesis: String,
    pub verdict: String,
    pub basis: String,
    pub e_value: Option<f64>,
    pub level: Option<f64>,
    pub reason: Option<String>,
    pub clipped: Option<Vec<Clip>>,
}

/// The method every `Interval` names.
pub const INTERVAL_METHOD: &str = "inverted registered e-process";

/// One bound of a claim in one stratum, its registered test inverted over a grid of margins
/// (`analysis::intervals`). The method was chosen after the results were known, so it is
/// never registered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Interval {
    pub stratum: String,
    pub side: String,
    /// The registered margin the grid steps from; `None` in a card built before it was kept.
    #[serde(default)]
    pub margin: Option<f64>,
    pub estimate: f64,
    /// The tightest margin the test shows; `None` when no grid margin passes, or when the
    /// margins that pass are no interval (`contiguous` false).
    pub bound: Option<f64>,
    pub method: String,
    pub grid: IntervalGrid,
    pub contiguous: bool,
    pub registered: bool,
}

/// The margins tried: from `from` toward `to` in steps of `step`, both ends left out.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IntervalGrid {
    pub from: f64,
    pub to: f64,
    pub step: f64,
}

impl From<&IntervalResult> for Interval {
    fn from(r: &IntervalResult) -> Interval {
        let i = &r.inversion;
        Interval {
            stratum: r.stratum.clone(),
            side: r.side.clone(),
            margin: Some(r.margin),
            estimate: r.estimate,
            bound: i.margin.filter(|_| i.contiguous),
            method: INTERVAL_METHOD.into(),
            grid: IntervalGrid {
                from: i.grid.from,
                to: i.grid.to,
                step: i.grid.step,
            },
            contiguous: i.contiguous,
            registered: false,
        }
    }
}

/// How many units of one stratum a mean claim clipped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub stratum: String,
    pub below: u64,
    pub above: u64,
    pub units: u64,
}

/// One row of the deviation table. A signed row comes from the register record. A recorded
/// row comes from `deviations.jsonl`, which the box writes. An unsigned row comes from
/// `deviations.toml`, which a person writes. No key signs the last two.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mark", rename_all = "lowercase")]
pub enum DeviationRow {
    Signed {
        from: String,
        reason: String,
        timing: String,
        differs: Vec<String>,
    },
    Recorded {
        kind: String,
        reason: String,
        timing: Timing,
        original: String,
        justification: String,
        impact: String,
        /// SHA-256 of `deviations.jsonl`.
        sha256: String,
    },
    Unsigned {
        /// `deviation` or `unregistered`.
        entry: String,
        /// The deviation's type, or the unregistered step.
        subject: String,
        reason: String,
        timing: Timing,
        original: Option<String>,
        justification: Option<String>,
        impact: Option<String>,
        source: String,
        /// SHA-256 of `deviations.toml`.
        sha256: String,
    },
}

/// The deviation table: signed rows, then the rows the box recorded, then the file's rows.
pub fn deviation_table(
    signed: &[PinDeviated],
    recorded: Option<&(Vec<Recorded>, String)>,
    file: Option<&Deviations>,
) -> Vec<DeviationRow> {
    let signed = signed.iter().map(|d| DeviationRow::Signed {
        from: d.from.clone(),
        reason: d.reason.clone(),
        timing: d.timing.clone(),
        differs: d.differs.clone(),
    });
    let recorded = recorded.into_iter().flat_map(|(rows, hash)| {
        rows.iter().map(move |r| DeviationRow::Recorded {
            kind: r.kind.clone(),
            reason: r.reason.clone(),
            timing: r.timing,
            original: r.original.clone(),
            justification: r.justification.clone(),
            impact: r.impact.clone(),
            sha256: hash.clone(),
        })
    });
    let unsigned = file.into_iter().flat_map(|f| {
        let hash = &f.sha256;
        let deviations = f.deviation.iter().map(move |d| DeviationRow::Unsigned {
            entry: "deviation".into(),
            subject: d.row.kind.clone(),
            reason: d.row.reason.clone(),
            timing: d.row.timing,
            original: Some(d.row.original.clone()),
            justification: Some(d.row.justification.clone()),
            impact: Some(d.row.impact.clone()),
            source: d.source.clone(),
            sha256: hash.clone(),
        });
        let unregistered = f.unregistered.iter().map(move |u| DeviationRow::Unsigned {
            entry: "unregistered".into(),
            subject: u.step.clone(),
            reason: u.reason.clone(),
            timing: u.timing,
            original: None,
            justification: None,
            impact: None,
            source: u.source.clone(),
            sha256: hash.clone(),
        });
        deviations.chain(unregistered)
    });
    signed.chain(recorded).chain(unsigned).collect()
}

/// The box that built the card, which may be a later box than the one that ran the rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Builder {
    pub version: Field<String>,
    pub binary_sha256: Field<String>,
}

impl Builder {
    /// The running binary, as the identity module hashed it. A binary it cannot read leaves
    /// the hash missing.
    fn this() -> Builder {
        let hash = match crate::identity::binary_sha256() {
            crate::telemetry::Reading::Read(h) => h.strip_prefix("sha256:"),
            crate::telemetry::Reading::Unavailable(_) => None,
        };
        Builder {
            version: Field::of(Some(env!("CARGO_PKG_VERSION").into()), None),
            binary_sha256: Field::of(hash.map(String::from), None),
        }
    }

    /// A card built before the builder was recorded.
    fn unknown() -> Builder {
        Builder {
            version: Field::of(None, None),
            binary_sha256: Field::of(None, None),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub card_version: u32,
    pub study: String,
    pub finished_at: String,
    pub records: u64,
    pub opened: Opened,
    pub hypotheses: Vec<HypothesisCard>,
    pub children: Vec<Value>,
    pub family: Value,
    pub deviations: Vec<DeviationRow>,
    pub header: Header,
    pub claims: Vec<ClaimLine>,
    /// The candidate the selection stage picked; `None` without a selection stage.
    #[serde(default)]
    pub pick: Option<sections::Pick>,
    pub verdicts: Vec<VerdictLine>,
    pub results: Vec<ResultRow>,
    pub sensitivity: Vec<Sensitivity>,
    pub setup: Setup,
    /// The observations each registered test read, in `observations.csv`.
    pub observations: Observations,
    pub surprises: Surprises,
    pub cost: Cost,
    pub runs: Runs,
    pub outside: OutsideFiles,
    /// The git commit that pins the outside files; `None` when no study folder was given.
    pub outside_source: Option<sections::Source>,
    pub recipe: Recipe,
    pub citation: Citation,
    #[serde(default = "Builder::unknown")]
    pub builder: Builder,
    /// The box that registered the plan, beside the one that built the card.
    #[serde(default = "sections::missing")]
    pub registered_box: Field<crate::identity::BoxIdentity>,
}

impl Card {
    /// The frozen card file: pretty JSON in struct order, one trailing newline.
    pub fn json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a card holds only string keys") + "\n"
    }

    /// The card's identity: the SHA-256 of `json()`.
    pub fn sha256(&self) -> String {
        crate::canon::sha256_hex(self.json().as_bytes())
    }
}

/// The time the results were complete: the later of the last counted estimate or verdict
/// record and, when the study holds a stream slot, the close of that slot. A record the report
/// drops as a repeat does not count. With neither, the analysis record's time.
pub(crate) fn finished_at(close: Option<String>, analysed: &str, after: &[Record]) -> String {
    let mut stamps: Vec<String> = counted(after).map(|r| r.ts.clone()).collect();
    stamps.extend(close);
    stamps
        .into_iter()
        .max()
        .unwrap_or_else(|| analysed.to_string())
}

/// The estimate and verdict records of `after` that count, in log order: the first of each
/// estimate identity and of each hypothesis's verdict.
pub(crate) fn counted(after: &[Record]) -> impl Iterator<Item = &Record> {
    let (mut estimates, mut verdicts) = (BTreeSet::new(), BTreeSet::new());
    after
        .iter()
        .filter(move |r| match (Estimate::of(r), Verdict::of(r)) {
            (Some(e), _) => estimates.insert(e.identity()),
            (_, Some(v)) => verdicts.insert(v.hypothesis),
            _ => false,
        })
}

/// How a finished study finished: its records after the analysis, and when.
struct Finish<'a> {
    analysed: &'a Record,
    after: Vec<Record>,
    at: String,
}

/// The finish of study `id`, whose records are `all`; `None` when it is not finished.
fn standing<'a>(
    records: &HomeRecords,
    id: &str,
    all: &'a [Record],
) -> Result<Option<Finish<'a>>, String> {
    let mut close = None;
    let state = StudyState::of(Some(all.to_vec()), |log, slot| {
        close = records.stream_closed_at(log, id, slot)?;
        Ok(close.is_some())
    })?;
    let StudyState::Finished(after) = state else {
        return Ok(None);
    };
    let analysed = (all.iter().find(|r| Analysis::of(r).is_some()))
        .ok_or_else(|| format!("study {id} has no analysis record"))?;
    let at = finished_at(close, &analysed.ts, &after);
    Ok(Some(Finish {
        analysed,
        after,
        at,
    }))
}

/// The family of study `id`, whose register record holds `own`: its root and every other
/// member, root first, each with its state. `at` is when `id` finished.
fn family_block(
    records: &HomeRecords,
    id: &str,
    own: &Register,
    at: &str,
) -> Result<Value, String> {
    let (root, lock) = match &own.family {
        Some(lock) => (id.to_string(), lock.clone()),
        None => match records.family_of(id)? {
            Some(found) => (found.root, found.lock),
            None => return Ok(Value::Null),
        },
    };
    let others = std::iter::once(root.clone())
        .chain(lock.members.into_iter().map(|m| m.id))
        .filter(|m| m != id);
    let mut members = vec![];
    for other in others {
        let fault = |e: String| format!("study {other}: {e}");
        let mut m = json!({
            "study": other, "state": "not registered",
            "finished_at": null, "first_opened": null, "before": null,
        });
        if let Some(all) = records.study(&other).map_err(fault)? {
            m["state"] = json!("not finished");
            if let Some(f) = standing(records, &other, &all).map_err(fault)? {
                m["state"] = json!("finished");
                m["first_opened"] =
                    json!((all.iter().find(|r| r.kind == OPENED)).map(|r| r.ts.clone()));
                m["before"] = json!(f.at.as_str() < at);
                m["finished_at"] = json!(f.at);
            }
        }
        members.push(m);
    }
    Ok(json!({ "root": root, "members": members }))
}

fn clips(analysis: &Analysis, h: &str) -> Option<Vec<Clip>> {
    let mine: Vec<Clip> = (analysis.clipped.iter())
        .filter(|c| c.hypothesis == h)
        .map(|c| Clip {
            stratum: c.stratum.clone(),
            below: c.below,
            above: c.above,
            units: c.units,
        })
        .collect();
    (!mine.is_empty()).then_some(mine)
}

/// The card of finished study `id`, whose log is `log`. Its outside files come from
/// `study_folder`, the author's git folder. Refuses a study that is not finished.
pub fn build(
    home: &Home,
    id: &str,
    log: &Log,
    study_folder: Option<&Path>,
) -> Result<Card, CliError> {
    let key = crate::keys::load(home)
        .map_err(|e| CliError::usage(format!("box key: {e}")))?
        .ok_or_else(|| not_registered(id))?;
    let all = &log.records;
    if let Some(r) = all.iter().find(|r| r.study != id) {
        return Err(CliError::refuse(format!(
            "cannot report {id}: record {} belongs to study {}",
            r.seq, r.study
        )));
    }
    let (registered, own) = (all.iter().find_map(|r| Register::of(r).map(|g| (r, g))))
        .ok_or_else(|| not_registered(id))?;
    let box_records = HomeRecords::signed(home, &key);
    let refuse = |e: String| CliError::refuse(format!("cannot report {id}: {e}"));
    let Some(Finish {
        analysed,
        after,
        at,
    }) = standing(&box_records, id, all).map_err(refuse)?
    else {
        let said = match stopped_why(all) {
            Some(why) => format!("study {id} is stopped: {why}; it has no report"),
            None => format!("study {id} is not finished; status shows its progress"),
        };
        return Err(CliError::refuse(said));
    };
    let analysis = Analysis::of(analysed).ok_or_else(|| not_registered(id))?;
    let verdicts: Vec<Verdict> = sections::first_verdicts(&after);
    let dir = home.study_dir(id);
    let recorded = crate::deviations::recorded(&dir).map_err(CliError::usage)?;
    let (outside, outside_source) = sections::outside(study_folder).map_err(CliError::usage)?;
    let held = |name| study_folder.filter(|_| outside.holds(name));
    let file = (held(crate::deviations::FILE))
        .map(crate::deviations::load)
        .transpose()
        .map_err(CliError::usage)?
        .flatten();
    let events = Outside::of(&dir, sections::EVENTS).map_err(CliError::usage)?;
    let text = std::fs::read(dir.join("spec.toml"))
        .map_err(|e| CliError::usage(format!("cannot report {id}: its plan: {e}")))?;
    let source = Source::read(&text, all, log.data_hashes.as_ref()).map_err(refuse)?;
    let intervals = source.intervals(&verdicts).map_err(refuse)?;
    let forced = sidecar::forced(&verdicts);
    let hypotheses: Vec<HypothesisCard> = (verdicts.into_iter())
        .map(|v| HypothesisCard {
            clipped: clips(&analysis, &v.hypothesis),
            hypothesis: v.hypothesis,
            verdict: v.verdict,
            basis: v.basis,
            e_value: v.e_value,
            level: v.level,
            reason: v.reason,
        })
        .collect();
    let mut seen = BTreeSet::new();
    let kids: Vec<ChildEstimate> = (all.iter().filter_map(ChildEstimate::of))
        .filter(|e| seen.insert((e.child.clone(), e.outcome.clone())))
        .collect();
    let shown = if kids.is_empty() {
        vec![]
    } else {
        shown_labels(&source.spec, &source.hash)
    };
    let stopped: BTreeSet<String> = (all.iter().filter_map(SpawnStop::of))
        .flat_map(|s| s.children)
        .collect();
    let children: Vec<Value> = (kids.iter())
        .map(|e| {
            json!({
                "child": relabelled(&shown, &e.child),
                "parent": relabelled(&shown, &e.parent),
                "operator": e.operator, "value": e.value, "outcome": e.outcome,
                "clusters": e.clusters, "units": e.units, "mean": e.mean, "repeats": e.repeats,
                "stopped": stopped.contains(&e.child),
            })
        })
        .collect();
    let family = family_block(&box_records, id, &own, &at).map_err(refuse)?;
    let spec = &source.spec;
    let sidecars = Sidecars::from_source(&source, log).map_err(refuse)?;
    let setup = sections::setup(log, &own, spec, events.as_ref(), sidecars.data_list());
    let levels = match &setup.stack.isolation {
        Field::Recorded { value } => value.as_slice(),
        _ => &[],
    };
    let (walked, lacks) = recipe::walk(&dir).map_err(CliError::usage)?;
    let signed = &sidecars.lines;
    let recipe = recipe::recipe(&walked, lacks, signed, id, levels, &analysis.box_version);
    let citation = recipe::citation(held(recipe::CITATION_FILE), id).map_err(CliError::usage)?;
    Ok(Card {
        header: sections::header(&own, &registered.ts, spec),
        claims: sections::claims(&after, spec),
        pick: sections::pick(log),
        verdicts: sections::verdicts(&after),
        results: sections::results(&after, spec, &intervals),
        sensitivity: sections::sensitivity(&own, &after, spec),
        setup,
        observations: sidecars.observations_card(forced),
        surprises: sections::surprises(log),
        recipe,
        citation,
        builder: Builder::this(),
        registered_box: Field::of(own.box_identity.clone(), None),
        cost: sections::cost(log, spec),
        runs: sections::runs(log, events.as_ref()),
        outside,
        outside_source,
        card_version: CARD_VERSION,
        study: id.to_string(),
        finished_at: at,
        records: all.len() as u64,
        opened: Opened::default(),
        hypotheses,
        children,
        family,
        deviations: deviation_table(&own.deviations, recorded.as_ref(), file.as_ref()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use evidence::claim::{Grid, Inversion};

    #[test]
    fn a_gap_in_the_passing_margins_prints_no_bound() {
        let result = |contiguous| IntervalResult {
            hypothesis: "H1".into(),
            stratum: String::new(),
            side: "below".into(),
            margin: 0.25,
            estimate: 0.0,
            inversion: Inversion {
                margin: Some(0.5),
                contiguous,
                grid: Grid {
                    from: 0.0,
                    to: 1.0,
                    step: 0.5,
                },
                threshold: 2.0,
            },
        };
        assert_eq!(Interval::from(&result(true)).bound, Some(0.5));
        assert_eq!(Interval::from(&result(true)).margin, Some(0.25));
        let gap = Interval::from(&result(false));
        assert_eq!((gap.bound, gap.contiguous), (None, false));
    }
}
