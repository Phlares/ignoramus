//! The only module with unsafe calls. Each is wrapped in a safe fn.
#![allow(unsafe_code)]
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Runs in signal context, so it only stores to an atomic, which is async-signal-safe.
extern "C" fn note_interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// SIGINT, SIGTERM and SIGHUP only set the flag `interrupted` reads. Each handler runs once:
/// a second signal of the same kind ends the process the default way. SIGCHLD goes back to
/// its default, since an inherited "ignore" lets the kernel reap a child before `has_exited`
/// sees it.
pub fn catch_interrupts() -> io::Result<()> {
    let note = note_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t;
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        set_action(sig, note, libc::SA_RESTART | libc::SA_RESETHAND)?;
    }
    set_action(libc::SIGCHLD, libc::SIG_DFL, 0)
}

/// True once SIGINT, SIGTERM or SIGHUP arrived after `catch_interrupts`.
pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

fn set_action(sig: libc::c_int, handler: libc::sighandler_t, flags: libc::c_int) -> io::Result<()> {
    // SAFETY: an all-zero `sigaction` is a valid value for this plain C struct.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handler;
    action.sa_flags = flags;
    // SAFETY: `sa_mask` is a live, writable `sigset_t`; emptying it cannot fail.
    unsafe { libc::sigemptyset(&mut action.sa_mask) };
    // SAFETY: `action` is a valid action whose handler is async-signal-safe, and a null
    // pointer for the old action is allowed.
    if unsafe { libc::sigaction(sig, &action, std::ptr::null_mut()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

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
    let r = loop {
        // SAFETY: `status` and `ru` are live, writable locals of the types wait4 expects.
        let r = unsafe { libc::wait4(pid as libc::pid_t, &mut status, flags, &mut ru) };
        if r >= 0 {
            break r;
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    };
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

/// True once child `pid` has exited. The child stays unreaped, so its pid and
/// process group cannot be reused until `wait4` collects it.
pub fn has_exited(pid: u32) -> io::Result<bool> {
    use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
    let pid = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad process id"))?;
    loop {
        let opts = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
        match waitid(WaitId::Pid(pid), opts) {
            Ok(st) => return Ok(st.is_some()),
            Err(e) if e == rustix::io::Errno::INTR => {}
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::thread::JoinHandleExt;
    use std::time::Duration;

    /// The action installed for `sig`.
    fn action_of(sig: libc::c_int) -> libc::sigaction {
        // SAFETY: an all-zero `sigaction` is a valid value for this plain C struct.
        let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
        // SAFETY: a null new action only reads the current one into `old`.
        assert_eq!(
            unsafe { libc::sigaction(sig, std::ptr::null(), &mut old) },
            0
        );
        old
    }

    #[test]
    fn interrupt_handlers_set_the_flag_once_then_fall_back_to_the_default() {
        catch_interrupts().unwrap();
        let note = note_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t;
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            let a = action_of(sig);
            assert_eq!(a.sa_sigaction, note, "{sig}");
            assert_ne!(a.sa_flags & libc::SA_RESTART, 0, "{sig}");
            assert_ne!(a.sa_flags & libc::SA_RESETHAND, 0, "{sig}");
        }
        assert_eq!(action_of(libc::SIGCHLD).sa_sigaction, libc::SIG_DFL);
        assert!(!interrupted());
        // SAFETY: the handler for SIGHUP only stores to an atomic.
        assert_eq!(unsafe { libc::raise(libc::SIGHUP) }, 0);
        assert!(interrupted());
        assert_eq!(action_of(libc::SIGHUP).sa_sigaction, libc::SIG_DFL);
    }

    extern "C" fn ignore(_: libc::c_int) {}

    #[test]
    fn wait4_waits_again_when_a_signal_interrupts_it() {
        // A handler without SA_RESTART, so a blocked wait4 returns EINTR.
        let handler = ignore as extern "C" fn(libc::c_int) as libc::sighandler_t;
        set_action(libc::SIGUSR1, handler, 0).unwrap();
        #[allow(clippy::zombie_processes)] // the wait4 under test reaps it
        let child = std::process::Command::new("sleep")
            .arg("1")
            .spawn()
            .unwrap();
        let pid = child.id();
        let waiter = std::thread::spawn(move || wait4(pid, true));
        // The waiter blocks in wait4 well before this; the signal lands mid-wait.
        std::thread::sleep(Duration::from_millis(300));
        // SAFETY: the waiter thread is alive until it is joined below.
        assert_eq!(
            unsafe { libc::pthread_kill(waiter.as_pthread_t(), libc::SIGUSR1) },
            0
        );
        let reaped = waiter.join().unwrap().unwrap().unwrap();
        assert_eq!(reaped.ended, Ended::Code(0));
    }
}
