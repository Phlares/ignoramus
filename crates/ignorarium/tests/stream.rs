mod common;

use common::{Lab, lab};
use ed25519_dalek::SigningKey;
use evidence::elond::Gamma;
use ignorarium::backend::level0::Level0;
use ignorarium::cli::check::HomeRecords;
use ignorarium::derived::{Analysis, StudyState};
use ignorarium::keys;
use ignorarium::records::{self, Record, RecordDraft};
use ignorarium::rules::BoxRecords;
use ignorarium::runner::{RunStatus, prepare_with, run};
use ignorarium::stream::{self, Log, Next, StreamError};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Barrier;

/// The first slot's level: 0.05 x 1/10 x (0 discoveries + 1), as f64 arithmetic gives it.
const FIRST: f64 = 0.005000000000000001;

fn key(lab: &Lab) -> SigningKey {
    keys::load_or_create(&lab.home).unwrap()
}

fn path(lab: &Lab, log: &str) -> PathBuf {
    lab.home.root.join("stream").join(format!("{log}.jsonl"))
}

fn all(lab: &Lab, log: &str) -> Vec<Record> {
    records::read_all(&path(lab, log), &key(lab).verifying_key()).unwrap()
}

fn kinds(lab: &Lab, log: &str) -> Vec<String> {
    all(lab, log).into_iter().map(|r| r.kind).collect()
}

fn bytes(lab: &Lab, log: &str) -> Vec<u8> {
    std::fs::read(path(lab, log)).unwrap_or_default()
}

#[test]
fn the_first_slot_gets_0_005() {
    assert_eq!(stream::ALPHA, 0.05);
    assert_eq!(stream::GAMMA, Gamma::Bounded { m: 10 });
    let lab = lab();
    let k = key(&lab);
    let log = Log::of(&lab.home, "planlog");
    // Asking writes nothing.
    let first = Next {
        slot: 1,
        level: FIRST,
    };
    assert_eq!(log.next(&k.verifying_key()).unwrap(), first);
    assert!(!path(&lab, "planlog").exists());
    assert_eq!(stream::shown(FIRST), "0.005");
    assert_eq!(log.open(&k, "first", FIRST).unwrap(), first);
    // The config comes before the first slot, signed into the stream's own log.
    let all = all(&lab, "planlog");
    assert_eq!(kinds(&lab, "planlog"), ["stream.config", "stream.open"]);
    assert_eq!(
        (all[0].study.as_str(), &all[0].body),
        ("", &json!({ "alpha": 0.05, "gamma": "bounded", "m": 10 }))
    );
    assert_eq!(
        (all[1].study.as_str(), &all[1].body),
        (
            "first",
            &json!({ "log": "planlog", "study": "first", "slot": 1, "level": FIRST })
        )
    );
    // The next study gets slot 2 at the same level, and no second config.
    let second = Next {
        slot: 2,
        level: FIRST,
    };
    assert_eq!(log.next(&k.verifying_key()).unwrap(), second);
    assert_eq!(log.open(&k, "second", FIRST).unwrap(), second);
    assert_eq!(
        kinds(&lab, "planlog"),
        ["stream.config", "stream.open", "stream.open"]
    );
}

#[test]
fn a_discovery_doubles_the_next_level() {
    let lab = lab();
    let k = key(&lab);
    let vk = k.verifying_key();
    let log = Log::of(&lab.home, "planlog");
    log.open(&k, "a", FIRST).unwrap();
    log.open(&k, "b", FIRST).unwrap();
    // 1 / 0.005 = 200: an e-value of 200 is a discovery.
    assert!(log.close(&k, "a", 1, 200.0).unwrap());
    let doubled = 0.010000000000000002;
    assert_eq!(
        log.next(&vk).unwrap(),
        Next {
            slot: 3,
            level: doubled
        }
    );
    assert_eq!(stream::shown(doubled), "0.01");
    // Slot 2 opened before the discovery: its level stays 0.005, and 199.9 misses it.
    assert!(!log.close(&k, "b", 2, 199.9).unwrap());
    assert_eq!(
        log.next(&vk).unwrap(),
        Next {
            slot: 3,
            level: doubled
        }
    );
    let closes: Vec<Value> = all(&lab, "planlog")
        .into_iter()
        .filter(|r| r.kind == "stream.close")
        .map(|r| r.body)
        .collect();
    assert_eq!(
        closes,
        [
            json!({ "slot": 1, "study": "a", "e_value": 200.0, "discovery": true }),
            json!({ "slot": 2, "study": "b", "e_value": 199.9, "discovery": false }),
        ]
    );
    assert_eq!(log.open(&k, "c", doubled).unwrap().level, doubled);
}

#[test]
fn the_eleventh_slot_is_refused() {
    let lab = lab();
    let k = key(&lab);
    let log = Log::of(&lab.home, "planlog");
    for i in 1..=10 {
        let got = log.open(&k, &format!("s{i:02}"), FIRST).unwrap();
        assert_eq!(
            got,
            Next {
                slot: i,
                level: FIRST
            }
        );
    }
    let beyond = Next {
        slot: 11,
        level: 0.0,
    };
    assert_eq!(log.next(&k.verifying_key()).unwrap(), beyond);
    assert_eq!(stream::shown(0.0), "0");
    let before = bytes(&lab, "planlog");
    // Level 0 is refused even when the check asked for no more.
    let e = log.open(&k, "s11", 0.0).unwrap_err();
    assert!(
        matches!(e, StreamError::Lower { slot: 11, level, at_least } if level == 0.0 && at_least == 0.0),
        "{e:?}"
    );
    assert_eq!(bytes(&lab, "planlog"), before);
}

#[test]
fn concurrent_opens_get_distinct_slots() {
    let lab = lab();
    let k = key(&lab);
    for round in 0..32 {
        let name = format!("round-{round}");
        let log = Log::of(&lab.home, &name);
        let gate = Barrier::new(2);
        let mut slots: Vec<u64> = std::thread::scope(|s| {
            let handles: Vec<_> = ["a", "b"]
                .into_iter()
                .map(|study| {
                    let (log, k, gate) = (&log, &k, &gate);
                    s.spawn(move || {
                        gate.wait();
                        log.open(k, study, FIRST).unwrap().slot
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        slots.sort();
        assert_eq!(slots, [1, 2], "round {round}");
        assert_eq!(
            kinds(&lab, &name),
            ["stream.config", "stream.open", "stream.open"],
            "round {round}"
        );
    }
}

#[test]
fn a_level_lower_than_checked_writes_nothing() {
    let lab = lab();
    let k = key(&lab);
    let log = Log::of(&lab.home, "planlog");
    // On an empty stream, no config is written either.
    let e = log.open(&k, "a", 2.0 * FIRST).unwrap_err();
    assert!(
        matches!(e, StreamError::Lower { slot: 1, level, at_least } if level == FIRST && at_least == 2.0 * FIRST),
        "{e:?}"
    );
    assert_eq!(bytes(&lab, "planlog"), b"");
    // The level the check showed passes, and a lower one too.
    assert_eq!(log.open(&k, "a", FIRST).unwrap().slot, 1);
    assert_eq!(log.open(&k, "b", 0.001).unwrap().slot, 2);
    let before = bytes(&lab, "planlog");
    assert!(matches!(
        log.open(&k, "c", 0.01),
        Err(StreamError::Lower { .. })
    ));
    assert_eq!(bytes(&lab, "planlog"), before);
}

#[test]
fn a_test_study_uses_the_test_stream() {
    assert_eq!(stream::log_for(true), "testlog");
    assert_eq!(stream::log_for(false), "planlog");
    let lab = lab();
    let k = key(&lab);
    let tests = Log::of(&lab.home, "testlog");
    assert_eq!(tests.open(&k, "trial", FIRST).unwrap().slot, 1);
    assert_eq!(all(&lab, "testlog")[1].body["log"], json!("testlog"));
    // The plan log's stream is untouched: its first slot is still free.
    assert!(!path(&lab, "planlog").exists());
    let plans = Log::of(&lab.home, "planlog");
    assert_eq!(plans.next(&k.verifying_key()).unwrap().slot, 1);
    // A test stream's records copied over the plan stream do not replay there.
    std::fs::copy(path(&lab, "testlog"), path(&lab, "planlog")).unwrap();
    assert!(matches!(
        plans.next(&k.verifying_key()),
        Err(StreamError::Replay { seq: 1, .. })
    ));
}

/// A record to plant: study, kind and body.
type Planted = (String, &'static str, Value);

/// Sign `bodies` into the stream log `planlog` by hand.
fn plant(lab: &Lab, bodies: &[Planted]) {
    let k = key(lab);
    std::fs::create_dir_all(path(lab, "planlog").parent().unwrap()).unwrap();
    for (study, kind, body) in bodies {
        let draft = RecordDraft {
            study: study.clone(),
            row: None,
            kind: kind.to_string(),
            body: body.clone(),
        };
        records::append(&path(lab, "planlog"), &k, draft).unwrap();
    }
}

fn config() -> Planted {
    let body = json!({ "alpha": 0.05, "gamma": "bounded", "m": 10 });
    (String::new(), "stream.config", body)
}

fn opened(study: &str, slot: u64, level: f64) -> Planted {
    let body = json!({ "log": "planlog", "study": study, "slot": slot, "level": level });
    (study.into(), "stream.open", body)
}

fn closed(slot: u64, study: &str, e: f64, discovery: bool) -> Planted {
    let body = json!({ "slot": slot, "study": study, "e_value": e, "discovery": discovery });
    (study.into(), "stream.close", body)
}

/// The record with its study set to `study` and its body unchanged.
fn by(study: &str, (_, kind, body): Planted) -> Planted {
    (study.into(), kind, body)
}

#[test]
fn a_config_record_that_differs_is_an_error() {
    let config_with = |key: &str, v: Value| {
        let mut c = config();
        c.2[key] = v;
        vec![c]
    };
    let cases = [
        config_with("alpha", json!(0.1)),
        config_with("m", json!(20)),
        config_with("extra", json!(1)),
        vec![opened("a", 1, FIRST)],
    ];
    for bodies in cases {
        let lab = lab();
        let k = key(&lab);
        plant(&lab, &bodies);
        let log = Log::of(&lab.home, "planlog");
        let before = bytes(&lab, "planlog");
        assert!(
            matches!(log.next(&k.verifying_key()), Err(StreamError::Config)),
            "{bodies:?}"
        );
        assert!(matches!(log.open(&k, "b", 0.0), Err(StreamError::Config)));
        assert_eq!(bytes(&lab, "planlog"), before);
    }
}

#[test]
fn a_record_that_does_not_follow_from_the_replay_is_an_error() {
    let open_a = || opened("a", 1, FIRST);
    let cases: Vec<(&str, Vec<Planted>)> = vec![
        ("a second config", vec![config()]),
        (
            "an unknown kind",
            vec![("a".into(), "stream.note", json!({}))],
        ),
        ("a skipped slot", vec![opened("a", 2, FIRST)]),
        ("another level", vec![opened("a", 1, 0.01)]),
        ("a body for another study", vec![by("b", open_a())]),
        ("a slot never opened", vec![closed(1, "a", 1.0, false)]),
        (
            "a slot closed twice",
            vec![
                open_a(),
                closed(1, "a", 1.0, false),
                closed(1, "a", 1.0, false),
            ],
        ),
        (
            "a close by another study",
            vec![open_a(), closed(1, "b", 1.0, false)],
        ),
        (
            "a close body for another study",
            vec![open_a(), by("b", closed(1, "a", 1.0, false))],
        ),
        (
            "a decision the e-value does not give",
            vec![open_a(), closed(1, "a", 1.0, true)],
        ),
        (
            "an invalid e-value",
            vec![open_a(), closed(1, "a", -1.0, false)],
        ),
    ];
    for (what, rest) in cases {
        let lab = lab();
        let k = key(&lab);
        let bodies: Vec<Planted> = std::iter::once(config()).chain(rest).collect();
        plant(&lab, &bodies);
        let last = bodies.len() as u64 - 1;
        let got = Log::of(&lab.home, "planlog").next(&k.verifying_key());
        assert!(
            matches!(got, Err(StreamError::Replay { seq, .. }) if seq == last),
            "{what}: {got:?}"
        );
    }
}

#[test]
fn a_slot_closes_once_with_a_finite_e_value() {
    let lab = lab();
    let k = key(&lab);
    let vk = k.verifying_key();
    let log = Log::of(&lab.home, "planlog");
    log.open(&k, "a", FIRST).unwrap();
    let before = bytes(&lab, "planlog");
    // Another study's slot, a slot never opened, and e-values a record cannot hold.
    assert!(matches!(
        log.close(&k, "b", 1, 1.0),
        Err(StreamError::NotOpen { slot: 1 })
    ));
    assert!(matches!(
        log.close(&k, "a", 2, 1.0),
        Err(StreamError::NotOpen { slot: 2 })
    ));
    for e in [f64::NAN, -1.0, f64::INFINITY] {
        assert!(
            matches!(log.close(&k, "a", 1, e), Err(StreamError::EValue)),
            "{e}"
        );
    }
    assert_eq!(bytes(&lab, "planlog"), before);
    assert!(!log.closed(&vk, "a", 1).unwrap());
    // e = 0 closes the slot without a discovery; a second close is refused.
    assert!(!log.close(&k, "a", 1, 0.0).unwrap());
    assert!(log.closed(&vk, "a", 1).unwrap());
    assert!(!log.closed(&vk, "b", 1).unwrap());
    assert!(!log.closed(&vk, "a", 2).unwrap());
    let closed = bytes(&lab, "planlog");
    assert!(matches!(
        log.close(&k, "a", 1, 0.0),
        Err(StreamError::NotOpen { slot: 1 })
    ));
    assert_eq!(bytes(&lab, "planlog"), closed);
}

#[test]
fn a_box_reads_its_streams_and_needs_a_key_only_for_a_log_that_exists() {
    let lab = lab();
    let records = |key| HomeRecords {
        home: lab.home.clone(),
        key,
    };
    // No stream log and no key: the stream is empty.
    let first = Next {
        slot: 1,
        level: FIRST,
    };
    assert_eq!(records(Ok(None)).stream_next("planlog"), Ok(first));
    assert_eq!(
        records(Ok(None)).stream_closed("planlog", "a", 1),
        Ok(false)
    );
    let k = key(&lab);
    Log::of(&lab.home, "planlog").open(&k, "a", FIRST).unwrap();
    let missing = "the box key is missing".to_string();
    assert_eq!(
        records(Ok(None)).stream_next("planlog"),
        Err(missing.clone())
    );
    assert_eq!(
        records(Ok(None)).stream_closed("planlog", "a", 1),
        Err(missing)
    );
    let broken = records(Err("key file has 5 bytes, expected 32".into()));
    assert_eq!(
        broken.stream_next("planlog"),
        Err("key file has 5 bytes, expected 32".to_string())
    );
    let with_key = records(Ok(Some(k.verifying_key())));
    assert_eq!(
        with_key.stream_next("planlog"),
        Ok(Next {
            slot: 2,
            level: FIRST
        })
    );
    Log::of(&lab.home, "planlog")
        .close(&k, "a", 1, 3.0)
        .unwrap();
    assert_eq!(with_key.stream_closed("planlog", "a", 1), Ok(true));
    assert_eq!(with_key.stream_closed("planlog", "b", 1), Ok(false));
    // A damaged stream log is an error.
    std::fs::write(path(&lab, "planlog"), "{}\n").unwrap();
    assert_eq!(
        with_key.stream_next("planlog"),
        Err("the log fails verification: line 1 is not a whole record".to_string())
    );
}

#[test]
fn a_confirmatory_study_finishes_only_once_its_slot_closes() {
    let lab = lab();
    common::pilot(&lab.home, vec![common::arm_a()]);
    common::register_study(&lab, "conf", common::confirmatory, &[], common::PAYLOAD);
    let plan = prepare_with(&lab.home, "conf", vec![Box::new(Level0)]).unwrap();
    assert_eq!(run(&plan).unwrap(), RunStatus::Done);
    let all = common::records_of(&lab, "conf");
    let analysis = all.iter().find_map(Analysis::of).unwrap();
    assert_eq!(analysis.slot, Some(1));
    let k = key(&lab);
    let records = HomeRecords {
        home: lab.home.clone(),
        key: Ok(Some(k.verifying_key())),
    };
    let state = || {
        let all = records.study("conf").unwrap();
        StudyState::of(all, |log, slot| records.stream_closed(log, "conf", slot)).unwrap()
    };
    assert_eq!(state(), StudyState::NotFinished);
    Log::of(&lab.home, "planlog")
        .close(&k, "conf", 1, 0.5)
        .unwrap();
    assert!(matches!(state(), StudyState::Finished(_)));
}
