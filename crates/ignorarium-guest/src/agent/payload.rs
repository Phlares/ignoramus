//! Runs the payload: a clean environment, no stdio, its own process group, a deadline.
use crate::proto::{End, Job, SEED_ENV};
use crate::sys;
use rustix::process::{Pid, Signal, WaitId, WaitIdOptions, kill_process_group, waitid};
use std::io;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

pub struct Ran {
    pub end: End,
    pub wall: Duration,
    pub cpu_seconds: f64,
}

/// Runs `job.argv` as the current user, in `/`. When this returns, no process of the
/// payload's group is left.
pub fn run(job: &Job) -> io::Result<Ran> {
    run_as(job, None, Path::new("/"))
}

/// Runs `job.argv` in the folder `cwd`, as `user` (uid, gid) when given. Changing the user
/// needs root.
pub fn run_as(job: &Job, user: Option<(u32, u32)>, cwd: &Path) -> io::Result<Ran> {
    run_watched(job, user, cwd, exited)
}

/// `run_as` with the exit check passed in, so a test can make it fail.
fn run_watched(
    job: &Job,
    user: Option<(u32, u32)>,
    cwd: &Path,
    mut exited: impl FnMut(Pid) -> io::Result<bool>,
) -> io::Result<Ran> {
    let Some((program, args)) = job.argv.split_first() else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty argv"));
    };
    let cpu_before = sys::children_cpu_seconds()?;
    let mut cmd = Command::new(program);
    cmd.args(args).env_clear().envs(job.env.iter().cloned());
    cmd.env(SEED_ENV, &job.seed_hex)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    if let Some((uid, gid)) = user {
        // When root changes the uid, std also drops the supplementary groups.
        cmd.uid(uid).gid(gid);
    }
    let start = Instant::now();
    let mut child = cmd.spawn()?;
    let pid = Pid::from_raw(child.id() as i32).ok_or_else(|| io::Error::other("bad pid"))?;
    let waited = timed_out(
        pid,
        start,
        Duration::from_millis(job.timeout_ms),
        &mut exited,
    );
    // On every way out of the wait, a failed check included. The leader is not reaped yet (a
    // zombie if it exited), so its pid still names our group and cannot be reused by an
    // unrelated one. A leader that moved to another group is not in it, so it is killed by
    // pid too.
    let _ = kill_process_group(pid, Signal::KILL);
    let _ = child.kill();
    let status = child.wait()?;
    let wall = start.elapsed();
    let end = if waited? {
        End::Timeout
    } else if let Some(sig) = status.signal() {
        End::Signal(sig)
    } else {
        End::Code(status.code().unwrap_or(-1))
    };
    let cpu_seconds = sys::children_cpu_seconds()? - cpu_before;
    Ok(Ran {
        end,
        wall,
        cpu_seconds,
    })
}

/// Polls `exited` until the payload exits (`false`) or `deadline` after `start` passes (`true`).
fn timed_out(
    pid: Pid,
    start: Instant,
    deadline: Duration,
    exited: &mut impl FnMut(Pid) -> io::Result<bool>,
) -> io::Result<bool> {
    loop {
        if exited(pid)? {
            return Ok(false);
        }
        if start.elapsed() >= deadline {
            return Ok(true);
        }
        sleep(Duration::from_millis(10));
    }
}

/// True once the process has exited. It stays unreaped (`WNOWAIT`).
pub(crate) fn exited(pid: Pid) -> io::Result<bool> {
    let opts = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
    Ok(waitid(WaitId::Pid(pid), opts)?.is_some())
}

#[cfg(test)]
mod tests {
    use super::run_watched;
    use crate::proto::Job;
    use std::fs;
    use std::io;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    /// True once the process is gone or a zombie, waiting up to 2 s for the kill to land.
    fn dies(pid: &str) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(2) {
            match fs::read_to_string(format!("/proc/{pid}/stat")) {
                Err(_) => return true,
                Ok(s)
                    if s.rsplit(')')
                        .next()
                        .is_some_and(|r| r.trim_start().starts_with('Z')) =>
                {
                    return true;
                }
                Ok(_) => sleep(Duration::from_millis(10)),
            }
        }
        false
    }

    #[test]
    fn a_failed_exit_check_still_kills_the_payload() {
        let d = tempfile::tempdir().unwrap();
        let bg = d.path().join("bg");
        let job = Job {
            seed_hex: "00".repeat(32),
            unix_nanos: 0,
            argv: ["/bin/sh", "-c", "sleep 30 & echo $! > \"$OUT/bg\"; wait"]
                .map(String::from)
                .to_vec(),
            env: vec![("OUT".into(), d.path().to_str().unwrap().into())],
            timeout_ms: 30_000,
        };
        let mut leader = None;
        let err = run_watched(&job, None, std::path::Path::new("/"), |pid| {
            leader = Some(pid);
            // Fail once the background child exists, so the group kill has a member to reach.
            let t = Instant::now();
            while !fs::read_to_string(&bg).is_ok_and(|s| s.ends_with('\n')) {
                assert!(t.elapsed() < Duration::from_secs(5), "no background child");
                sleep(Duration::from_millis(5));
            }
            Err(io::Error::other("wait failed"))
        })
        .err()
        .unwrap();
        assert_eq!(err.to_string(), "wait failed");
        let bg_pid = fs::read_to_string(&bg).unwrap();
        assert!(dies(bg_pid.trim()), "the payload's group survived");
        let leader = leader.unwrap().as_raw_nonzero().to_string();
        assert!(dies(&leader), "the leader survived");
    }
}
