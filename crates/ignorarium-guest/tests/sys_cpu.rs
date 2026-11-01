//! Alone in its process: reaped children of the test process all count, so another test
//! could only add to the reading. The upper bound is loose and the lower bound tight.
use ignorarium_guest::agent::payload;
use ignorarium_guest::proto::Job;

fn spin(n: u64) -> Job {
    Job {
        seed_hex: "00".repeat(32),
        unix_nanos: 0,
        argv: [
            "/bin/sh".to_string(),
            "-c".to_string(),
            format!("i=0; while [ $i -lt {n} ]; do i=$((i+1)); done"),
        ]
        .to_vec(),
        env: vec![],
        timeout_ms: 60_000,
        data_limit: None,
    }
}

#[test]
fn cpu_seconds_of_a_busy_second_is_about_a_second() {
    // Size the loop from a short run so it takes about 1.2 s here.
    let probe = 200_000;
    let w0 = payload::run(&spin(probe)).unwrap().wall.as_secs_f64();
    let n = (probe as f64 * 1.2 / w0) as u64;
    let ran = payload::run(&spin(n)).unwrap();
    let wall = ran.wall.as_secs_f64();
    assert!(wall > 0.8, "the loop took only {wall} s");
    assert!(
        ran.cpu_seconds >= 0.8 * wall,
        "{} s of CPU in {wall} s",
        ran.cpu_seconds
    );
    assert!(ran.cpu_seconds < 60.0, "{}", ran.cpu_seconds);
}
