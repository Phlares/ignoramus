//! The box-wide e-LOND stream that gives each confirmatory study its level. Each log
//! (`planlog`, `testlog`) has its own stream in `stream/<log>.jsonl`: a signed record log
//! whose first record fixes alpha and gamma, then one `stream.open` per slot, in the order
//! studies register, and one `stream.close` per slot that a study's e-value has closed. The
//! state is rebuilt by replaying those records into an `evidence::elond::Stream`. The plan
//! stream gives levels to M slots and then only level 0. The test stream never runs out:
//! after each M slots a `stream.epoch` record starts a fresh stream, with slot numbers that
//! go on counting.
use crate::derived::Alpha;
use crate::home::Home;
use crate::records::{self, Record, RecordDraft, RecordError, VerifyError};
use ed25519_dalek::{SigningKey, VerifyingKey};
use evidence::elond::{Gamma, Slot, Stream};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The false discovery rate the stream keeps over the studies it counts as discoveries.
pub const ALPHA: f64 = 0.05;
/// The slots that share alpha; a slot after them has level 0.
pub const M: u64 = 10;
/// Weight 1/M for each of the first M slots.
pub const GAMMA: Gamma = Gamma::Bounded { m: M };

pub const CONFIG: &str = "stream.config";
pub const EPOCH: &str = "stream.epoch";
pub const OPEN: &str = "stream.open";
pub const CLOSE: &str = "stream.close";

/// The log a study's plan leaf and stream slot go to: the plan log, or the test log, which
/// never counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogName {
    Plan,
    Test,
}

impl LogName {
    /// The log `study.test_log` names.
    pub fn of(test_log: bool) -> LogName {
        if test_log {
            LogName::Test
        } else {
            LogName::Plan
        }
    }

    /// `planlog` or `testlog`; any other name is an error, so no stream path is built from it.
    pub fn parse(name: &str) -> Result<LogName, StreamError> {
        [LogName::Plan, LogName::Test]
            .into_iter()
            .find(|l| l.as_str() == name)
            .ok_or(StreamError::Name)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LogName::Plan => "planlog",
            LogName::Test => "testlog",
        }
    }
}

/// The body of the first record: the stream's constants.
fn config() -> Value {
    json!({ "alpha": ALPHA, "gamma": "bounded", "m": M })
}

/// A slot of the stream, and the level it gives the study that opens it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Next {
    pub slot: u64,
    pub level: f64,
}

#[derive(Debug)]
pub enum StreamError {
    /// A log name other than `planlog` and `testlog`.
    Name,
    /// The log cannot be read, trusted or appended to.
    Records(RecordError),
    /// The first record is not a `stream.config` with this box's alpha and gamma.
    Config,
    /// Record `seq` does not follow from the records before it.
    Replay { seq: u64, why: &'static str },
    /// Slot `slot` gives `level`: 0, or below the `at_least` the check showed.
    Lower {
        slot: u64,
        level: f64,
        at_least: f64,
    },
    /// Slot `slot` is not open for the study: never opened, held by another study, or closed.
    NotOpen { slot: u64 },
    /// An e-value that is NaN, negative or infinite.
    EValue,
    /// A signed `register` record names slot `slot` of `study` at `level`, and the stream
    /// holds no such slot: the log was cut, replaced or restored from an older copy.
    Lacks {
        study: String,
        slot: u64,
        level: f64,
    },
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamError::Name => write!(f, "a stream log is planlog or testlog"),
            StreamError::Records(e) => write!(f, "{e}"),
            StreamError::Config => write!(
                f,
                "the first record is not the config of alpha {ALPHA} over {M} slots"
            ),
            StreamError::Replay { seq, why } => write!(f, "stream record {seq} {why}"),
            StreamError::Lower {
                slot,
                level,
                at_least,
            } => write!(
                f,
                "slot {slot} has level {}, below the {} the check showed",
                shown(*level),
                shown(*at_least)
            ),
            StreamError::NotOpen { slot } => write!(f, "slot {slot} is not open for this study"),
            StreamError::EValue => write!(f, "the e-value is not a finite number of 0 or more"),
            StreamError::Lacks { study, slot, level } => write!(
                f,
                "the stream lacks slot {slot} of study {study} at level {}",
                shown(*level)
            ),
        }
    }
}

impl std::error::Error for StreamError {}

impl From<RecordError> for StreamError {
    fn from(e: RecordError) -> Self {
        StreamError::Records(e)
    }
}

impl From<VerifyError> for StreamError {
    fn from(e: VerifyError) -> Self {
        StreamError::Records(RecordError::Log(e))
    }
}

/// A level as output prints it: six decimals at most, trailing zeros dropped, so the first
/// slot's level (0.05 x 0.1 in floating point) prints as 0.005.
pub fn shown(level: f64) -> String {
    let s = format!("{level:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Open {
    log: String,
    study: String,
    slot: u64,
    level: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Close {
    slot: u64,
    study: String,
    e_value: f64,
    discovery: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Epoch {
    epoch: u64,
}

/// The stream the records of `log` rebuild. Slots are numbered over the whole log; each epoch
/// is a fresh e-LOND stream, and only the test log has more than one.
struct Replay {
    log: LogName,
    /// One stream per epoch, the current one last.
    epochs: Vec<Stream>,
    /// Slots opened so far, over every epoch.
    opened: u64,
    /// Each open slot: its epoch's slot, the epoch, and the study that holds it.
    open: BTreeMap<u64, (Slot, usize, String)>,
    /// Every slot opened, with its study and level.
    given: BTreeMap<u64, (String, f64)>,
}

impl Replay {
    fn new(log: LogName) -> Replay {
        Replay {
            log,
            epochs: vec![fresh()],
            opened: 0,
            open: BTreeMap::new(),
            given: BTreeMap::new(),
        }
    }

    /// True when the next slot starts a new epoch: the test stream's current epoch has given
    /// its M slots. The plan stream never starts one, so its slots after M get level 0.
    fn epoch_due(&self) -> bool {
        self.log == LogName::Test && self.epochs.last().is_some_and(|s| s.opened() >= M)
    }

    fn start_epoch(&mut self) {
        self.epochs.push(fresh());
    }

    /// Open the next slot in the current epoch: its number over the log, and the slot.
    fn open(&mut self) -> (u64, Slot) {
        let current = self.epochs.len() - 1;
        let slot = self.epochs[current].open();
        self.opened += 1;
        (self.opened, slot)
    }

    /// Open the slot a study would get now, after starting the epoch that is due, if one is:
    /// whether an epoch started, the slot's number over the log, and the slot.
    fn open_next(&mut self) -> (bool, u64, Slot) {
        let epoch = self.epoch_due();
        if epoch {
            self.start_epoch();
        }
        let (n, slot) = self.open();
        (epoch, n, slot)
    }

    /// True when `study` holds or held slot `slot` at `level`.
    fn gave(&self, study: &str, slot: u64, level: f64) -> bool {
        (self.given.get(&slot)).is_some_and(|(s, l)| s == study && l.to_bits() == level.to_bits())
    }
}

fn fresh() -> Stream {
    Stream::new(ALPHA, GAMMA).expect("the stream constants are valid")
}

/// Replay `all`, the records of the stream of `log`. Every record must be the one the
/// stream gives at that point: the config first, each slot in order at its epoch's level, an
/// epoch only when one is due, each close of an open slot by its study with the decision the
/// e-value gives in that slot's epoch.
fn replay(log: LogName, all: &[Record]) -> Result<Replay, StreamError> {
    let mut r = Replay::new(log);
    let Some((first, rest)) = all.split_first() else {
        return Ok(r);
    };
    if first.kind != CONFIG || first.body != config() {
        return Err(StreamError::Config);
    }
    for rec in rest {
        let bad = |why| StreamError::Replay { seq: rec.seq, why };
        match rec.kind.as_str() {
            EPOCH => {
                let e: Epoch = serde_json::from_value(rec.body.clone())
                    .map_err(|_| bad("is not a stream.epoch body"))?;
                if !r.epoch_due() || e.epoch != r.epochs.len() as u64 + 1 || !rec.study.is_empty() {
                    return Err(bad("starts an epoch the stream does not start"));
                }
                r.start_epoch();
            }
            OPEN => {
                let o: Open = serde_json::from_value(rec.body.clone())
                    .map_err(|_| bad("is not a stream.open body"))?;
                let (n, slot) = r.open();
                if (o.log.as_str(), o.study.as_str()) != (log.as_str(), rec.study.as_str())
                    || o.slot != n
                    || o.level.to_bits() != slot.level().to_bits()
                {
                    return Err(bad("is not the slot the stream opens next"));
                }
                r.given.insert(n, (o.study.clone(), o.level));
                r.open.insert(n, (slot, r.epochs.len() - 1, o.study));
            }
            CLOSE => {
                let c: Close = serde_json::from_value(rec.body.clone())
                    .map_err(|_| bad("is not a stream.close body"))?;
                let (slot, epoch, _) = r
                    .open
                    .remove(&c.slot)
                    .filter(|(_, _, s)| *s == c.study && c.study == rec.study)
                    .ok_or(bad("closes a slot the study does not hold open"))?;
                let discovery = r.epochs[epoch]
                    .close(slot, c.e_value)
                    .map_err(|_| bad("holds an invalid e-value"))?;
                if discovery != c.discovery {
                    return Err(bad("records a decision its e-value does not give"));
                }
            }
            _ => return Err(bad("is not a stream record")),
        }
    }
    Ok(r)
}

/// What the stream of `log`, after the records `all`, would give the next slot. `held` is
/// the alpha of each signed `register` record, by study: a slot one of them names for this
/// log must be in the stream, given to that study at that level, or the log was cut,
/// replaced or restored from an older copy.
pub fn next_of(
    log: LogName,
    all: &[Record],
    held: &[(String, Alpha)],
) -> Result<Next, StreamError> {
    let mut r = replay(log, all)?;
    let lacking = (held.iter())
        .filter(|(_, a)| a.log == log.as_str())
        .find(|(study, a)| !r.gave(study, a.slot, a.level));
    if let Some((study, a)) = lacking {
        return Err(StreamError::Lacks {
            study: study.clone(),
            slot: a.slot,
            level: a.level,
        });
    }
    let (_, slot, s) = r.open_next();
    Ok(Next {
        slot,
        level: s.level(),
    })
}

/// The stream of one log.
pub struct Log {
    path: PathBuf,
    log: LogName,
}

impl Log {
    pub fn of(home: &Home, log: LogName) -> Log {
        Log {
            path: home
                .root
                .join("stream")
                .join(format!("{}.jsonl", log.as_str())),
            log,
        }
    }

    /// The file that holds the stream's records.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn replayed(&self, key: &VerifyingKey) -> Result<Replay, StreamError> {
        replay(self.log, &records::read_all(&self.path, key)?)
    }

    /// Open the next slot for `study`, under the log's lock: the config first when the log
    /// is empty, then the slot with its level. Refused, with nothing written, when the level
    /// is 0 or below `at_least`.
    pub fn open(&self, key: &SigningKey, study: &str, at_least: f64) -> Result<Next, StreamError> {
        let mut opened = None;
        records::append_with(&self.path, key, |all| -> Result<_, StreamError> {
            let mut r = replay(self.log, all)?;
            let (epoch, index, slot) = r.open_next();
            let level = slot.level();
            if !(level > 0.0 && level >= at_least) {
                return Err(StreamError::Lower {
                    slot: index,
                    level,
                    at_least,
                });
            }
            let mut drafts = Vec::new();
            if all.is_empty() {
                drafts.push(RecordDraft::new("", CONFIG, config()));
            }
            if epoch {
                drafts.push(RecordDraft::new(
                    "",
                    EPOCH,
                    json!({ "epoch": r.epochs.len() }),
                ));
            }
            let log = self.log.as_str();
            let body = json!({ "log": log, "study": study, "slot": index, "level": level });
            drafts.push(RecordDraft::new(study, OPEN, body));
            opened = Some(Next { slot: index, level });
            Ok(drafts)
        })?;
        Ok(opened.expect("a slot is opened whenever the append succeeds"))
    }

    /// Close slot `slot`, which `study` holds open, with the study's e-value `e`, under the
    /// log's lock. A slot closes once. Returns whether it is a discovery: `e` at least 1 /
    /// the slot's level.
    pub fn close(
        &self,
        key: &SigningKey,
        study: &str,
        slot: u64,
        e: f64,
    ) -> Result<bool, StreamError> {
        if !(e.is_finite() && e >= 0.0) {
            return Err(StreamError::EValue);
        }
        let mut decided = None;
        records::append_with(&self.path, key, |all| -> Result<_, StreamError> {
            let mut r = replay(self.log, all)?;
            let (held, epoch, _) = (r.open.remove(&slot))
                .filter(|(_, _, holder)| holder.as_str() == study)
                .ok_or(StreamError::NotOpen { slot })?;
            let discovery = r.epochs[epoch]
                .close(held, e)
                .map_err(|_| StreamError::EValue)?;
            let body =
                json!({ "slot": slot, "study": study, "e_value": e, "discovery": discovery });
            decided = Some(discovery);
            Ok(vec![RecordDraft::new(study, CLOSE, body)])
        })?;
        Ok(decided.expect("a decision is made whenever the append succeeds"))
    }

    /// True when `study` held slot `slot` and the slot is closed.
    pub fn closed(&self, key: &VerifyingKey, study: &str, slot: u64) -> Result<bool, StreamError> {
        let r = self.replayed(key)?;
        let held = r.given.get(&slot).is_some_and(|(s, _)| s == study);
        Ok(held && !r.open.contains_key(&slot))
    }
}
