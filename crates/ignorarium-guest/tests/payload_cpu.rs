//! Alone in its process: the CPU time of reaped children is shared by every thread, so a
//! parallel test would add to the second reading.
use ignorarium_guest::agent::payload;
use ignorarium_guest::proto::Job;

fn sh(script: &str) -> Job {
    Job {
        seed_hex: "00".repeat(32),
        unix_nanos: 0,
        argv: ["/bin/sh", "-c", script].map(String::from).to_vec(),
        env: vec![],
        timeout_ms: 20_000,
        data_limit: None,
    }
}

#[test]
fn cpu_seconds_counts_only_the_payload_that_ran() {
    let spin = "i=0; while [ $i -lt 1500000 ]; do i=$((i+1)); done";
    let first = payload::run(&sh(spin)).unwrap().cpu_seconds;
    assert!(first > 0.1, "{first}");
    let second = payload::run(&sh("exit 0")).unwrap().cpu_seconds;
    assert!(second < 0.05, "{second} counts the first payload ({first})");
}
