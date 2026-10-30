//! The box-wide e-LOND stream that gives each confirmatory study its level. Each log
//! (`planlog`, `testlog`) has its own stream in `stream/<log>.jsonl`: a signed record log
//! whose first record fixes alpha and gamma, then one `stream.open` per slot, in the order
//! studies register, and one `stream.close` per slot that a study's e-value has closed. The
//! state is rebuilt by replaying those records into an `evidence::elond::Stream`.
use crate::home::Home;
use crate::records::{self, Record, RecordDraft, RecordError, VerifyError};
use ed25519_dalek::{SigningKey, VerifyingKey};
use evidence::elond::{Gamma, Slot, Stream};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The false discovery rate the stream keeps over every claim it confirms.
pub const ALPHA: f64 = 0.05;
/// The slots that share alpha; a slot after them has level 0.
pub const M: u64 = 10;
/// Weight 1/M for each of the first M slots.
pub const GAMMA: Gamma = Gamma::Bounded { m: M };

pub const CONFIG: &str = "stream.config";
pub const OPEN: &str = "stream.open";
pub const CLOSE: &str = "stream.close";

/// The log a study's plan leaf and stream slot go to.
pub fn log_for(test_log: bool) -> &'static str {
    if test_log { "testlog" } else { "planlog" }
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
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
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

/// The stream the records of `log` rebuild.
struct Replay {
    stream: Stream,
    /// Each open slot, with the study that holds it.
    open: BTreeMap<u64, (Slot, String)>,
    /// Each closed slot, with the study that held it.
    closed: BTreeMap<u64, String>,
}

/// Replay `all`, the records of the stream of `log`. Every record must be the one the
/// stream gives at that point: the config first, each slot in order at the stream's level,
/// each close of an open slot by its study with the decision the e-value gives.
fn replay(log: &str, all: &[Record]) -> Result<Replay, StreamError> {
    let mut r = Replay {
        stream: Stream::new(ALPHA, GAMMA).expect("the stream constants are valid"),
        open: BTreeMap::new(),
        closed: BTreeMap::new(),
    };
    let Some((first, rest)) = all.split_first() else {
        return Ok(r);
    };
    if first.kind != CONFIG || first.body != config() {
        return Err(StreamError::Config);
    }
    for rec in rest {
        let bad = |why| StreamError::Replay { seq: rec.seq, why };
        match rec.kind.as_str() {
            OPEN => {
                let o: Open = serde_json::from_value(rec.body.clone())
                    .map_err(|_| bad("is not a stream.open body"))?;
                let slot = r.stream.open();
                if (o.log.as_str(), o.study.as_str()) != (log, rec.study.as_str())
                    || o.slot != slot.index()
                    || o.level.to_bits() != slot.level().to_bits()
                {
                    return Err(bad("is not the slot the stream opens next"));
                }
                r.open.insert(o.slot, (slot, o.study));
            }
            CLOSE => {
                let c: Close = serde_json::from_value(rec.body.clone())
                    .map_err(|_| bad("is not a stream.close body"))?;
                let (slot, study) = r
                    .open
                    .remove(&c.slot)
                    .filter(|(_, s)| *s == c.study && c.study == rec.study)
                    .ok_or(bad("closes a slot the study does not hold open"))?;
                let discovery = r
                    .stream
                    .close(slot, c.e_value)
                    .map_err(|_| bad("holds an invalid e-value"))?;
                if discovery != c.discovery {
                    return Err(bad("records a decision its e-value does not give"));
                }
                r.closed.insert(c.slot, study);
            }
            _ => return Err(bad("is not a stream record")),
        }
    }
    Ok(r)
}

/// What the stream of `log`, after the records `all`, would give the next slot.
pub fn next_of(log: &str, all: &[Record]) -> Result<Next, StreamError> {
    let slot = replay(log, all)?.stream.open();
    Ok(Next {
        slot: slot.index(),
        level: slot.level(),
    })
}

fn draft(study: &str, kind: &str, body: Value) -> RecordDraft {
    RecordDraft {
        study: study.into(),
        row: None,
        kind: kind.into(),
        body,
    }
}

/// The stream of one log.
pub struct Log {
    path: PathBuf,
    log: String,
}

impl Log {
    pub fn of(home: &Home, log: &str) -> Log {
        Log {
            path: home.root.join("stream").join(format!("{log}.jsonl")),
            log: log.into(),
        }
    }

    /// The file that holds the stream's records.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn replayed(&self, key: &VerifyingKey) -> Result<Replay, StreamError> {
        replay(&self.log, &records::read_all(&self.path, key)?)
    }

    /// The slot and level the stream would give a study that opened a slot now. Writes
    /// nothing.
    pub fn next(&self, key: &VerifyingKey) -> Result<Next, StreamError> {
        next_of(&self.log, &records::read_all(&self.path, key)?)
    }

    /// Open the next slot for `study`, under the log's lock: the config first when the log
    /// is empty, then the slot with its level. Refused, with nothing written, when the level
    /// is 0 or below `at_least`.
    pub fn open(&self, key: &SigningKey, study: &str, at_least: f64) -> Result<Next, StreamError> {
        let mut opened = None;
        records::append_with(&self.path, key, |all| -> Result<_, StreamError> {
            let slot = replay(&self.log, all)?.stream.open();
            let (index, level) = (slot.index(), slot.level());
            if !(level > 0.0 && level >= at_least) {
                return Err(StreamError::Lower {
                    slot: index,
                    level,
                    at_least,
                });
            }
            let mut drafts = Vec::new();
            if all.is_empty() {
                drafts.push(draft("", CONFIG, config()));
            }
            let body = json!({ "log": self.log, "study": study, "slot": index, "level": level });
            drafts.push(draft(study, OPEN, body));
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
            let mut r = replay(&self.log, all)?;
            let (held, _) = (r.open.remove(&slot))
                .filter(|(_, holder)| holder.as_str() == study)
                .ok_or(StreamError::NotOpen { slot })?;
            let discovery = r.stream.close(held, e).map_err(|_| StreamError::EValue)?;
            let body =
                json!({ "slot": slot, "study": study, "e_value": e, "discovery": discovery });
            decided = Some(discovery);
            Ok(vec![draft(study, CLOSE, body)])
        })?;
        Ok(decided.expect("a decision is made whenever the append succeeds"))
    }

    /// True when `study` held slot `slot` and the slot is closed.
    pub fn closed(&self, key: &VerifyingKey, study: &str, slot: u64) -> Result<bool, StreamError> {
        let r = self.replayed(key)?;
        Ok(r.closed.get(&slot).is_some_and(|s| s == study))
    }
}
