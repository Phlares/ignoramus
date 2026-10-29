//! The only module with unsafe calls. Each is wrapped in a safe fn.
#![allow(unsafe_code)]
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Runs in signal context, so it only stores to an atomic, which is async-signal-safe.
extern "C" fn note_interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// SIGINT, SIGTERM and SIGHUP only set the flag `interrupted` reads. A signal ignored on
/// entry stays ignored, as `nohup` and a shell's `&` ask. The SIGINT handler runs once, so a
/// second Ctrl-C ends the process the default way; SIGTERM and SIGHUP stay caught, since a
/// closing terminal sends SIGHUP twice. SIGCHLD goes back to its default, since an
/// inherited "ignore" lets the kernel reap a child before `has_exited` sees it.
pub fn catch_interrupts() -> io::Result<()> {
    let note = note_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t;
    let actions = [
        (libc::SIGINT, libc::SA_RESETHAND),
        (libc::SIGTERM, 0),
        (libc::SIGHUP, 0),
    ];
    for (sig, reset) in actions {
        if action_of(sig)?.sa_sigaction != libc::SIG_IGN {
            set_action(sig, note, libc::SA_RESTART | reset)?;
        }
    }
    set_action(libc::SIGCHLD, libc::SIG_DFL, 0)
}

/// True when the kernel reaps this process's children on its own: SIGCHLD is ignored, or
/// its action asks for no zombies. A child then vanishes before `has_exited` can see it end.
pub fn sigchld_ignored() -> io::Result<bool> {
    let a = action_of(libc::SIGCHLD)?;
    Ok(a.sa_sigaction == libc::SIG_IGN || a.sa_flags & libc::SA_NOCLDWAIT != 0)
}

/// The action installed for `sig` now.
fn action_of(sig: libc::c_int) -> io::Result<libc::sigaction> {
    // SAFETY: an all-zero `sigaction` is a valid value for this plain C struct.
    let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: a null new action only reads the current one into `old`, a live local.
    if unsafe { libc::sigaction(sig, std::ptr::null(), &mut old) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(old)
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

/// Make the child that `cmd` starts inherit, besides stdin, stdout and stderr, only the
/// descriptors in `keep`, at the same numbers. The box's own descriptors are close-on-exec,
/// but ones it inherited from its own parent may not be. A kernel before Linux 5.11 lacks
/// `CLOSE_RANGE_CLOEXEC`, so the spawn fails there.
pub fn inherit_only(cmd: &mut std::process::Command, keep: [i32; 2]) {
    use std::os::unix::process::CommandExt;
    let hook = move || -> io::Result<()> {
        let (first, last): (libc::c_uint, libc::c_uint) = (3, libc::c_uint::MAX);
        // SAFETY: close_range takes three integers and only marks this process's descriptors
        // close-on-exec; it reads and writes no memory.
        let marked = unsafe {
            libc::syscall(
                libc::SYS_close_range,
                first,
                last,
                libc::CLOSE_RANGE_CLOEXEC,
            )
        };
        if marked != 0 {
            return Err(io::Error::last_os_error());
        }
        for fd in keep {
            // SAFETY: fcntl F_SETFD with 0 clears close-on-exec on a descriptor number; it
            // reads and writes no memory.
            if unsafe { libc::fcntl(fd, libc::F_SETFD, 0) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    };
    // SAFETY: `hook` runs in the child between fork and exec. It makes only the
    // async-signal-safe close_range and fcntl calls, and it allocates nothing.
    unsafe { cmd.pre_exec(hook) };
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

    fn handler(sig: libc::c_int) -> libc::sighandler_t {
        action_of(sig).unwrap().sa_sigaction
    }

    fn flags(sig: libc::c_int) -> libc::c_int {
        action_of(sig).unwrap().sa_flags
    }

    /// One test, in order, since signal dispositions are process-wide.
    #[test]
    fn interrupt_handlers_keep_an_ignore_and_only_sigint_falls_back() {
        let note = note_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t;
        // A signal ignored on entry, as `nohup` leaves SIGHUP, stays ignored.
        set_action(libc::SIGHUP, libc::SIG_IGN, 0).unwrap();
        catch_interrupts().unwrap();
        assert_eq!(handler(libc::SIGHUP), libc::SIG_IGN);
        assert_eq!(handler(libc::SIGINT), note);
        assert_eq!(handler(libc::SIGTERM), note);
        assert_eq!(handler(libc::SIGCHLD), libc::SIG_DFL);
        set_action(libc::SIGHUP, libc::SIG_DFL, 0).unwrap();
        catch_interrupts().unwrap();
        assert_eq!(handler(libc::SIGHUP), note);
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            assert_ne!(flags(sig) & libc::SA_RESTART, 0, "{sig}");
        }
        assert_ne!(flags(libc::SIGINT) & libc::SA_RESETHAND, 0);
        assert_eq!(flags(libc::SIGTERM) & libc::SA_RESETHAND, 0);
        assert_eq!(flags(libc::SIGHUP) & libc::SA_RESETHAND, 0);
        assert!(!interrupted());
        // SAFETY: the handlers for SIGHUP and SIGINT only store to an atomic.
        assert_eq!(unsafe { libc::raise(libc::SIGHUP) }, 0);
        assert!(interrupted());
        // A second SIGHUP finds the handler still there.
        assert_eq!(handler(libc::SIGHUP), note);
        // SAFETY: as above.
        assert_eq!(unsafe { libc::raise(libc::SIGINT) }, 0);
        assert_eq!(handler(libc::SIGINT), libc::SIG_DFL);
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
