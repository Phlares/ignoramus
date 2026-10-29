use ignorarium::backend::level0::{Level0, output_hash};
use ignorarium::backend::{Backend, Exit, Probe, RowJob};
use sha2::{Digest, Sha256};
use std::time::Duration;

fn job(dir: &std::path::Path, script: &str, timeout: Duration) -> RowJob {
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    RowJob {
        argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
        env: vec![("OUT".into(), out.to_string_lossy().into())],
        input_dir: dir.join("in"),
        output_dir: out,
        seed: [0; 32],
        wall_timeout: timeout,
        rootfs: None,
    }
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

#[test]
fn level0_runs_hashes_output_and_times_the_row() {
    let d = tempfile::tempdir().unwrap();
    let o = Level0
        .run_row(&job(
            d.path(),
            "printf 42 > \"$OUT/result\"",
            Duration::from_secs(30),
        ))
        .unwrap();
    assert_eq!(o.exit, Exit::Ok);
    assert!(o.selftest_ok);
    assert!(o.wall_seconds > 0.0 && o.wall_seconds < 30.0);
    assert!(o.cpu_seconds >= 0.0 && o.cpu_seconds < 30.0);
    let manifest = format!("[[\"result\",\"file\",\"{}\"]]", sha(b"42"));
    assert_eq!(o.output_hash, sha(manifest.as_bytes()));
}

#[test]
fn level0_marks_a_nonzero_exit_a_payload_failure() {
    let d = tempfile::tempdir().unwrap();
    let o = Level0
        .run_row(&job(d.path(), "exit 3", Duration::from_secs(30)))
        .unwrap();
    assert_eq!(o.exit, Exit::Payload(3));
}

#[test]
fn level0_clears_the_environment() {
    let d = tempfile::tempdir().unwrap();
    let o = Level0
        .run_row(&job(
            d.path(),
            "tr '\\0' '\\n' </proc/$$/environ > \"$OUT/env\"",
            Duration::from_secs(30),
        ))
        .unwrap();
    assert_eq!(o.exit, Exit::Ok);
    let seen = std::fs::read_to_string(d.path().join("out/env")).unwrap();
    assert_eq!(
        seen,
        format!(
            "OUT={}/out
",
            d.path().display()
        )
    );
}

#[test]
fn level0_kills_the_group_at_the_wall_timeout() {
    let d = tempfile::tempdir().unwrap();
    let o = Level0
        .run_row(&job(
            d.path(),
            "(sleep 3; printf late > \"$OUT/late\") & sleep 30",
            Duration::from_millis(300),
        ))
        .unwrap();
    assert_eq!(o.exit, Exit::Payload(124));
    assert!(
        o.wall_seconds >= 0.3 && o.wall_seconds < 2.5,
        "{}",
        o.wall_seconds
    );
    std::thread::sleep(Duration::from_millis(3500));
    assert!(!d.path().join("out/late").exists());
}

#[test]
fn level0_reports_a_missing_program_as_infra() {
    let d = tempfile::tempdir().unwrap();
    let mut j = job(d.path(), "", Duration::from_secs(5));
    j.argv = vec!["/nonexistent/prog".into()];
    assert!(matches!(Level0.run_row(&j).unwrap().exit, Exit::Infra(_)));
    j.argv = vec![];
    assert_eq!(
        Level0.run_row(&j).unwrap().exit,
        Exit::Infra("empty argv".into())
    );
}

#[test]
fn level0_reports_a_signal_death_as_128_plus_signal() {
    let d = tempfile::tempdir().unwrap();
    let o = Level0
        .run_row(&job(d.path(), "kill -9 $$", Duration::from_secs(30)))
        .unwrap();
    assert_eq!(o.exit, Exit::Payload(137));
}

#[test]
fn level0_counts_cpu_time() {
    let d = tempfile::tempdir().unwrap();
    let o = Level0
        .run_row(&job(
            d.path(),
            "i=0; while [ $i -lt 300000 ]; do i=$((i+1)); done",
            Duration::from_secs(60),
        ))
        .unwrap();
    assert_eq!(o.exit, Exit::Ok);
    assert!(o.cpu_seconds > 0.0);
}

#[test]
fn level0_probe_and_level() {
    assert_eq!(Level0.probe(), Probe::Available);
    assert_eq!(Level0.level(), 0);
}

#[test]
fn output_hash_records_links_and_nested_files_without_following() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("a/b")).unwrap();
    std::fs::write(d.path().join("a/b/c"), b"x").unwrap();
    std::fs::write(d.path().join("z"), b"y").unwrap();
    std::os::unix::fs::symlink("/etc/hostname", d.path().join("a/ln")).unwrap();
    let m = format!(
        "[[\"a/b/c\",\"file\",\"{}\"],[\"a/ln\",\"link\",\"\"],[\"z\",\"file\",\"{}\"]]",
        sha(b"x"),
        sha(b"y")
    );
    assert_eq!(output_hash(d.path()).unwrap(), sha(m.as_bytes()));
}

#[test]
fn registered_starts_with_level0() {
    assert_eq!(ignorarium::backend::registered()[0].level(), 0);
}
