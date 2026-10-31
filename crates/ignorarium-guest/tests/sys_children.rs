//! Alone in its process: the sum under test is of every child this process has reaped.
use ignorarium_guest::sys::{children_cpu_seconds, wait4};
use std::process::Command;

fn secs(t: libc::timeval) -> f64 {
    t.tv_sec as f64 + t.tv_usec as f64 / 1e6
}

#[test]
fn children_cpu_is_user_plus_system() {
    // Each pass opens and closes a file (system time) and does arithmetic (user time).
    // `wait4` reaps the child, which `Child::wait` would hide from the resource usage.
    #[allow(clippy::zombie_processes)]
    let child = Command::new("sh")
        .args([
            "-c",
            "i=0; while [ $i -lt 30000 ]; do i=$((i+1)); : > /dev/null; done",
        ])
        .spawn()
        .unwrap();
    let reaped = wait4(child.id()).unwrap();
    // SAFETY: an all-zero rusage is a valid value, and getrusage fills it.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, &mut ru) },
        0
    );
    let (user, system) = (secs(ru.ru_utime), secs(ru.ru_stime));
    assert!(
        user > 0.0 && system > 0.0,
        "user {user} s, system {system} s"
    );
    let sum = children_cpu_seconds().unwrap();
    assert!(
        sum > user && sum > system,
        "{sum} s from {user} s and {system} s"
    );
    assert!(
        (sum - (user + system)).abs() < 1e-6,
        "{sum} s from {user} s and {system} s"
    );
    assert!((reaped.cpu_seconds - sum).abs() < 1e-6);
}
