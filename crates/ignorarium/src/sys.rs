//! The only module with unsafe calls. Each is wrapped in a safe fn.
#![allow(unsafe_code)]
use std::io;

/// How a child ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    Code(i32),
    Signal(i32),
}

/// Result of a non-blocking `wait4`.
pub struct Reaped {
    pub ended: Ended,
    pub cpu_seconds: f64,
}

fn secs(t: libc::timeval) -> f64 {
    t.tv_sec as f64 + t.tv_usec as f64 / 1e6
}

/// Reap child `pid`. With `block` false, returns `None` while it still runs.
/// CPU time is user plus system time from the child's resource usage.
pub fn wait4(pid: u32, block: bool) -> io::Result<Option<Reaped>> {
    let mut status: libc::c_int = 0;
    // SAFETY: an all-zero `rusage` is a valid value for this plain C struct.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    let flags = if block { 0 } else { libc::WNOHANG };
    // SAFETY: `status` and `ru` are live, writable locals of the types wait4 expects.
    let r = unsafe { libc::wait4(pid as libc::pid_t, &mut status, flags, &mut ru) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    if r == 0 {
        return Ok(None);
    }
    let ended = if libc::WIFEXITED(status) {
        Ended::Code(libc::WEXITSTATUS(status))
    } else {
        Ended::Signal(libc::WTERMSIG(status))
    };
    Ok(Some(Reaped {
        ended,
        cpu_seconds: secs(ru.ru_utime) + secs(ru.ru_stime),
    }))
}

/// Send SIGKILL to the process group led by `pid`. A group already gone is fine.
pub fn kill_group(pid: u32) -> io::Result<()> {
    use rustix::process::{Pid, Signal, kill_process_group};
    let pid = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad process id"))?;
    match kill_process_group(pid, Signal::KILL) {
        Ok(()) => Ok(()),
        Err(e) if e == rustix::io::Errno::SRCH => Ok(()),
        Err(e) => Err(e.into()),
    }
}
