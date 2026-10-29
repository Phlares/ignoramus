//! Runs the payload: a clean environment, no stdio, its own process group, a deadline.
use crate::proto::{End, Job, SEED_ENV};
use crate::sys;
use rustix::process::{Pid, Signal, WaitId, WaitIdOptions, kill_process_group, waitid};
use std::io;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

pub struct Ran {
    pub end: End,
    pub wall: Duration,
    pub cpu_seconds: f64,
}

/// Runs `job.argv` as the current user. When this returns, no process of the payload's
/// group is left.
pub fn run(job: &Job) -> io::Result<Ran> {
    run_as(job, None)
}

/// Runs `job.argv` as `user` (uid, gid) when given. Changing the user needs root.
pub fn run_as(job: &Job, user: Option<(u32, u32)>) -> io::Result<Ran> {
    let Some((program, args)) = job.argv.split_first() else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty argv"));
    };
    let cpu_before = sys::children_cpu_seconds()?;
    let mut cmd = Command::new(program);
    cmd.args(args).env_clear().envs(job.env.iter().cloned());
    cmd.env(SEED_ENV, &job.seed_hex)
        .current_dir("/")
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
    let deadline = Duration::from_millis(job.timeout_ms);
    let timed_out = loop {
        if exited(pid)? {
            break false;
        }
        if start.elapsed() >= deadline {
            break true;
        }
        sleep(Duration::from_millis(10));
    };
    // The leader is not reaped yet (a zombie if it exited), so its pid still names our group
    // and cannot be reused by an unrelated one. A leader that moved to another group is
    // not in it, so it is killed by pid too.
    let _ = kill_process_group(pid, Signal::KILL);
    let _ = child.kill();
    let status = child.wait()?;
    let wall = start.elapsed();
    let end = if timed_out {
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

/// True once the process has exited. It stays unreaped (`WNOWAIT`).
fn exited(pid: Pid) -> io::Result<bool> {
    let opts = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
    Ok(waitid(WaitId::Pid(pid), opts)?.is_some())
}
