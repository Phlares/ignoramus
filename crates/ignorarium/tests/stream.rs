mod common;

use common::{Lab, lab};
use ed25519_dalek::SigningKey;
use evidence::elond::Gamma;
use ignorarium::backend::level0::Level0;
use ignorarium::cli::check::HomeRecords;
use ignorarium::derived::{Alpha, Analysis, StudyState};
use ignorarium::keys;
use ignorarium::records::{self, Record, RecordDraft};
use ignorarium::rules::BoxRecords;
use ignorarium::runner::{RunStatus, prepare_with, run};
use ignorarium::stream::{self, Log, LogName, Next, StreamError};
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
    let log = Log::of(&lab.home, LogName::Plan);
    // Asking writes nothing.
    let first = Next {
        slot: 1,
        level: FIRST,
    };
    assert_eq!(common::next_slot(&lab.home, LogName::Plan, &k.verifying_key()).unwrap(), first);
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
    assert_eq!(common::next_slot(&lab.home, LogName::Plan, &k.verifying_key()).unwrap(), second);
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
    let log = Log::of(&lab.home, LogName::Plan);
    log.open(&k, "a", FIRST).unwrap();
    log.open(&k, "b", FIRST).unwrap();
    // 1 / 0.005 = 200: an e-value of 200 is a discovery.
    assert!(log.close(&k, "a", 1, 200.0).unwrap());
    let doubled = 0.010000000000000002;
    assert_eq!(
        common::next_slot(&lab.home, LogName::Plan, &vk).unwrap(),
        Next {
            slot: 3,
            level: doubled
        }
    );
    assert_eq!(stream::shown(doubled), "0.01");
    // Slot 2 opened before the discovery: its level stays 0.005, and 199.9 misses it.
    assert!(!log.close(&k, "b", 2, 199.9).unwrap());
    assert_eq!(
        common::next_slot(&lab.home, LogName::Plan, &vk).unwrap(),
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
    let log = Log::of(&lab.home, LogName::Plan);
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
    assert_eq!(common::next_slot(&lab.home, LogName::Plan, &k.verifying_key()).unwrap(), beyond);
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
    for round in 0..32 {
        let lab = lab();
        let k = key(&lab);
        let log = Log::of(&lab.home, LogName::Plan);
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
            kinds(&lab, "planlog"),
            ["stream.config", "stream.open", "stream.open"],
            "round {round}"
        );
    }
}

#[test]
fn a_level_lower_than_checked_writes_nothing() {
    let lab = lab();
    let k = key(&lab);
    let log = Log::of(&lab.home, LogName::Plan);
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
    assert_eq!(LogName::of(true), LogName::Test);
    assert_eq!(LogName::of(false), LogName::Plan);
    for log in [LogName::Plan, LogName::Test] {
        assert_eq!(LogName::parse(log.as_str()).unwrap(), log);
    }
    assert_eq!(LogName::Test.as_str(), "testlog");
    assert_eq!(LogName::Plan.as_str(), "planlog");
    // No other name builds a stream path.
    for name in ["", "../planlog", "Planlog", "round-1", "studies/x"] {
        assert!(
            matches!(LogName::parse(name), Err(StreamError::Name)),
            "{name}"
        );
    }
    let lab = lab();
    let k = key(&lab);
    let tests = Log::of(&lab.home, LogName::Test);
    assert_eq!(tests.open(&k, "trial", FIRST).unwrap().slot, 1);
    assert_eq!(all(&lab, "testlog")[1].body["log"], json!("testlog"));
    // The plan log's stream is untouched: its first slot is still free.
    assert!(!path(&lab, "planlog").exists());
    let plans = Log::of(&lab.home, LogName::Plan);
    assert_eq!(common::next_slot(&lab.home, LogName::Plan, &k.verifying_key()).unwrap().slot, 1);
    // A test stream's records copied over the plan stream do not replay there.
    std::fs::copy(path(&lab, "testlog"), path(&lab, "planlog")).unwrap();
    assert!(matches!(
        common::next_slot(&lab.home, LogName::Plan, &k.verifying_key()),
        Err(StreamError::Replay { seq: 1, .. })
    ));
}

/// A record to plant: study, kind and body.
type Planted = (String, &'static str, Value);

/// Sign `bodies` into the stream log `log` by hand.
fn plant(lab: &Lab, log: &str, bodies: &[Planted]) {
    let k = key(lab);
    for (study, kind, body) in bodies {
        let draft = RecordDraft {
            study: study.clone(),
            row: None,
            kind: kind.to_string(),
            body: body.clone(),
        };
        records::append(&path(lab, log), &k, draft).unwrap();
    }
}

fn config() -> Planted {
    let body = json!({ "alpha": 0.05, "gamma": "bounded", "m": 10 });
    (String::new(), "stream.config", body)
}

fn opened(study: &str, slot: u64, level: f64) -> Planted {
    opened_in("planlog", study, slot, level)
}

fn opened_in(log: &str, study: &str, slot: u64, level: f64) -> Planted {
    let body = json!({ "log": log, "study": study, "slot": slot, "level": level });
    (study.into(), "stream.open", body)
}

fn epoch(n: u64) -> Planted {
    (String::new(), "stream.epoch", json!({ "epoch": n }))
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
        plant(&lab, "planlog", &bodies);
        let log = Log::of(&lab.home, LogName::Plan);
        let before = bytes(&lab, "planlog");
        assert!(
            matches!(common::next_slot(&lab.home, LogName::Plan, &k.verifying_key()), Err(StreamError::Config)),
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
        plant(&lab, "planlog", &bodies);
        let last = bodies.len() as u64 - 1;
        let got = common::next_slot(&lab.home, LogName::Plan, &k.verifying_key());
        assert!(
            matches!(got, Err(StreamError::Replay { seq, .. }) if seq == last),
            "{what}: {got:?}"
        );
    }
}

#[test]
fn the_test_stream_starts_a_new_epoch_after_its_tenth_slot() {
    let lab = lab();
    let k = key(&lab);
    let vk = k.verifying_key();
    let log = Log::of(&lab.home, LogName::Test);
    for i in 1..=10 {
        log.open(&k, &format!("t{i:02}"), FIRST).unwrap();
    }
    // A discovery in the first epoch.
    assert!(log.close(&k, "t01", 1, 200.0).unwrap());
    // Slot 11 starts epoch 2 at the first level: the discovery stays in epoch 1.
    let first_of_two = Next {
        slot: 11,
        level: FIRST,
    };
    assert_eq!(common::next_slot(&lab.home, LogName::Test, &vk).unwrap(), first_of_two);
    assert_eq!(log.open(&k, "t11", FIRST).unwrap(), first_of_two);
    let all = all(&lab, "testlog");
    let tail: Vec<(&str, &Value)> = all[all.len() - 2..]
        .iter()
        .map(|r| (r.kind.as_str(), &r.body))
        .collect();
    assert_eq!(
        tail,
        [
            ("stream.epoch", &json!({ "epoch": 2 })),
            (
                "stream.open",
                &json!({ "log": "testlog", "study": "t11", "slot": 11, "level": FIRST })
            ),
        ]
    );
    // A slot of epoch 1 still closes, at its own level, and leaves epoch 2 alone.
    assert!(log.close(&k, "t02", 2, 200.0).unwrap());
    let second = Next {
        slot: 12,
        level: FIRST,
    };
    assert_eq!(common::next_slot(&lab.home, LogName::Test, &vk).unwrap(), second);
    // A discovery in epoch 2 doubles its next level.
    assert!(log.close(&k, "t11", 11, 200.0).unwrap());
    assert_eq!(common::next_slot(&lab.home, LogName::Test, &vk).unwrap().level, 2.0 * FIRST);
    assert!(log.closed(&vk, "t01", 1).unwrap());
    assert!(log.closed(&vk, "t11", 11).unwrap());
    assert!(!log.closed(&vk, "t03", 3).unwrap());
}

#[test]
fn an_epoch_starts_only_after_a_full_test_epoch_and_never_in_the_plan_stream() {
    let tens = |log: &str| -> Vec<Planted> {
        (1..=10)
            .map(|i| opened_in(log, &format!("t{i:02}"), i, FIRST))
            .collect()
    };
    let cases: Vec<(&str, &str, Vec<Planted>, u64)> = vec![
        (
            "an epoch before the tenth slot",
            "testlog",
            tens("testlog")[..9].to_vec(),
            2,
        ),
        ("a skipped epoch number", "testlog", tens("testlog"), 3),
        ("an epoch in the plan stream", "planlog", tens("planlog"), 2),
    ];
    for (what, log, slots, n) in cases {
        let lab = lab();
        let k = key(&lab);
        let mut bodies = vec![config()];
        bodies.extend(slots);
        bodies.push(epoch(n));
        plant(&lab, log, &bodies);
        let name = LogName::parse(log).unwrap();
        let got = common::next_slot(&lab.home, name, &k.verifying_key());
        let last = bodies.len() as u64 - 1;
        assert!(
            matches!(got, Err(StreamError::Replay { seq, .. }) if seq == last),
            "{what}: {got:?}"
        );
    }
    // The epoch the box writes replays.
    let lab = lab();
    let k = key(&lab);
    let mut bodies = vec![config()];
    bodies.extend(tens("testlog"));
    bodies.push(epoch(2));
    plant(&lab, "testlog", &bodies);
    let got = common::next_slot(&lab.home, LogName::Test, &k.verifying_key());
    assert_eq!(
        got.unwrap(),
        Next {
            slot: 11,
            level: FIRST
        }
    );
}

/// Sign a `register` record for study `id` that holds `alpha`, as `register` writes it.
fn registered(lab: &Lab, id: &str, alpha: Alpha) {
    let mut body = common::finished(vec![])[0].1.clone();
    body["alpha"] = serde_json::to_value(alpha).unwrap();
    common::plant(&lab.home, id, &[("register", body)]);
}

fn alpha(log: &str, slot: u64, level: f64) -> Alpha {
    Alpha {
        log: log.into(),
        slot,
        level,
    }
}

#[test]
fn a_stream_that_lacks_a_registered_slot_is_refused() {
    let lab = lab();
    let k = key(&lab);
    let records = HomeRecords {
        home: lab.home.clone(),
        key: Ok(Some(k.verifying_key())),
    };
    let log = Log::of(&lab.home, LogName::Plan);
    log.open(&k, "first", FIRST).unwrap();
    registered(&lab, "first", alpha("planlog", 1, FIRST));
    let kept = bytes(&lab, "planlog");
    log.open(&k, "second", FIRST).unwrap();
    registered(&lab, "second", alpha("planlog", 2, FIRST));
    assert_eq!(records.stream_next("planlog").unwrap().slot, 3);
    // An older copy of the stream: slot 2 is missing.
    let current = bytes(&lab, "planlog");
    std::fs::write(path(&lab, "planlog"), &kept).unwrap();
    assert_eq!(
        records.stream_next("planlog"),
        Err("the stream lacks slot 2 of study second at level 0.005".to_string())
    );
    // The stream log moved away: slot 1 is missing too.
    std::fs::rename(path(&lab, "planlog"), lab.root.join("moved.jsonl")).unwrap();
    assert_eq!(
        records.stream_next("planlog"),
        Err("the stream lacks slot 1 of study first at level 0.005".to_string())
    );
    std::fs::write(path(&lab, "planlog"), &current).unwrap();
    // A register record whose slot the stream gave another study.
    registered(&lab, "third", alpha("planlog", 1, FIRST));
    assert_eq!(
        records.stream_next("planlog"),
        Err("the stream lacks slot 1 of study third at level 0.005".to_string())
    );
    // A register record at another level than its slot's, in the test stream.
    let lab = common::lab();
    let k = key(&lab);
    let records = HomeRecords {
        home: lab.home.clone(),
        key: Ok(Some(k.verifying_key())),
    };
    Log::of(&lab.home, LogName::Test)
        .open(&k, "lone", FIRST)
        .unwrap();
    registered(&lab, "lone", alpha("testlog", 1, 0.01));
    assert_eq!(
        records.stream_next("testlog"),
        Err("the stream lacks slot 1 of study lone at level 0.01".to_string())
    );
    // Each stream answers for its own slots only.
    assert_eq!(records.stream_next("planlog").unwrap().slot, 1);
    // A study log whose first record cannot be verified is an error, not a study to skip.
    std::fs::create_dir_all(lab.home.study_dir("torn")).unwrap();
    std::fs::write(lab.home.study_dir("torn").join("records.jsonl"), "{}\n").unwrap();
    assert_eq!(
        records.stream_next("planlog"),
        Err("study torn: line 1 is not a whole record".to_string())
    );
}

#[test]
fn a_test_stream_rolled_back_past_its_first_epoch_is_refused() {
    let lab = lab();
    let k = key(&lab);
    let records = HomeRecords {
        home: lab.home.clone(),
        key: Ok(Some(k.verifying_key())),
    };
    let log = Log::of(&lab.home, LogName::Test);
    for i in 1..=10 {
        let id = format!("t{i:02}");
        let next = log.open(&k, &id, FIRST).unwrap();
        registered(&lab, &id, alpha("testlog", next.slot, next.level));
    }
    let first_epoch = bytes(&lab, "testlog");
    // Slot 11 opens epoch 2, and its register record names it by its number over the log.
    let next = log.open(&k, "t11", FIRST).unwrap();
    assert_eq!((next.slot, next.level), (11, FIRST));
    registered(&lab, "t11", alpha("testlog", 11, next.level));
    assert_eq!(records.stream_next("testlog").unwrap().slot, 12);
    // A copy from before epoch 2 lacks slot 11.
    std::fs::write(path(&lab, "testlog"), &first_epoch).unwrap();
    assert_eq!(
        records.stream_next("testlog"),
        Err("the stream lacks slot 11 of study t11 at level 0.005".to_string())
    );
}

#[test]
fn a_register_between_the_reads_of_a_check_is_no_lack() {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    // Study b's log is a FIFO, so check waits inside its read of the register records while
    // b opens slot 1 and writes the register record that names it. Check reads the stream
    // after that, so it finds slot 1 there.
    let lab = lab();
    let k = key(&lab);
    let records = HomeRecords {
        home: lab.home.clone(),
        key: Ok(Some(k.verifying_key())),
    };
    let scratch = lab.root.join("scratch.jsonl");
    let mut body = common::finished(vec![])[0].1.clone();
    body["alpha"] = serde_json::to_value(alpha("testlog", 1, FIRST)).unwrap();
    records::append(&scratch, &k, RecordDraft::new("b", "register", body)).unwrap();
    let line = std::fs::read(&scratch).unwrap();
    let log = lab.home.study_dir("b").join("records.jsonl");
    std::fs::create_dir_all(lab.home.study_dir("b")).unwrap();
    let made = std::process::Command::new("mkfifo").arg(&log).status();
    assert!(made.unwrap().success());
    let check = std::thread::spawn(move || records.stream_next("testlog"));
    // A write end opens only once check holds the read end.
    let mut fifo = loop {
        let opened = (std::fs::OpenOptions::new().write(true))
            .custom_flags(libc::O_NONBLOCK)
            .open(&log);
        match opened {
            Ok(f) => break f,
            Err(e) if e.raw_os_error() == Some(libc::ENXIO) && !check.is_finished() => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(e) => panic!("{e}; check: {:?}", check.join()),
        }
    };
    Log::of(&lab.home, LogName::Test).open(&k, "b", FIRST).unwrap();
    fifo.write_all(&line).unwrap();
    drop(fifo);
    assert_eq!(check.join().unwrap().unwrap().slot, 2);
}

#[test]
fn a_slot_closes_once_with_a_finite_e_value() {
    let lab = lab();
    let k = key(&lab);
    let vk = k.verifying_key();
    let log = Log::of(&lab.home, LogName::Plan);
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
    // A name a register record could carry builds no path outside the two streams.
    let named = "a stream log is planlog or testlog".to_string();
    assert_eq!(
        records(Ok(None)).stream_next("../studies/x"),
        Err(named.clone())
    );
    assert_eq!(
        records(Ok(None)).stream_closed("round-1", "a", 1),
        Err(named)
    );
    let k = key(&lab);
    Log::of(&lab.home, LogName::Plan)
        .open(&k, "a", FIRST)
        .unwrap();
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
    Log::of(&lab.home, LogName::Plan)
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
    let plan = prepare_with(&lab.home, "conf", &common::work_of(&lab.home, "conf"), vec![Box::new(Level0)]).unwrap();
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
    // The run closed the slot.
    assert!(matches!(state(), StudyState::Finished(_)));
    // A stream without the close, as a crash before it leaves one: not finished until the
    // slot closes.
    let stream = path(&lab, "planlog");
    let text = std::fs::read_to_string(&stream).unwrap();
    let open: String = text.split_inclusive('\n').take(2).collect();
    std::fs::write(&stream, open).unwrap();
    assert_eq!(state(), StudyState::NotFinished);
    Log::of(&lab.home, LogName::Plan)
        .close(&k, "conf", 1, 0.5)
        .unwrap();
    assert!(matches!(state(), StudyState::Finished(_)));
}
