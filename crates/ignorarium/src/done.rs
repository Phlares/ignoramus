//! The end of a study, as its `done` record states it.
use crate::card::{counted, finished_at};
use crate::derived::{ANALYSIS, By, CONTINUE, DoneRecord, End, StudyState};
use crate::records::{self, Record};

/// The end the log `all` holds, or `None` while it holds none. A finished study (`StudyState`)
/// ends by its rows, as a resize stop when its `resize` record does not continue, at the card's
/// `finished_at`; `slot_closed_at` is the close time of the stream slot it holds, `None` while
/// that slot is open. A `stop` record ends it at that record's time. When both hold, the end
/// whose record comes first in the log wins.
pub fn end_of(all: &[Record], slot_closed_at: Option<&str>) -> Option<DoneRecord> {
    let stop = (all.iter().find(|r| r.kind == "stop")).map(|r| (End::Stop, r.ts.clone(), r));
    let rows = finished(all, slot_closed_at);
    let (end, ended_at, by) = [rows, stop].into_iter().flatten().min_by_key(|e| e.2.seq)?;
    Some(DoneRecord {
        end,
        ended_at,
        by: By {
            seq: by.seq,
            hash: records::hash(by),
        },
    })
}

/// The end of a finished study, met by the last estimate or verdict it counts, else by its
/// analysis record. The analysis record is in `rest` and never counts.
fn finished<'a>(all: &'a [Record], close: Option<&str>) -> Option<(End, String, &'a Record)> {
    let state = StudyState::of(Some(all.to_vec()), |_, _| Ok(close.is_some()));
    if !matches!(state, Ok(StudyState::Finished(_))) {
        return None;
    }
    let at = all.iter().position(|r| r.kind == ANALYSIS)?;
    let rest = &all[at..];
    let by = counted(rest).last().unwrap_or(&all[at]);
    let end = match crate::resize::recorded(all).ok().flatten() {
        Some(r) if r.decision != CONTINUE => End::ResizeStop,
        _ => End::Rows,
    };
    let ended_at = finished_at(close.map(String::from), &all[at].ts, rest);
    Some((end, ended_at, by))
}
