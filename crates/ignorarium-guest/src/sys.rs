//! The only unsafe code in the crate: calls that rustix does not cover, behind safe functions.
#![allow(unsafe_code)]
use std::io;
use std::mem;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};

/// `_IO('R', 0x07)` from `<linux/random.h>`.
const RNDRESEEDCRNG: libc::c_ulong = 0x5207;

fn check(r: libc::c_int) -> io::Result<libc::c_int> {
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

/// Opens a stream socket to `cid:port` on the virtual socket bus.
pub fn vsock_connect(cid: u32, port: u32) -> io::Result<OwnedFd> {
    // SAFETY: plain socket(2); the result is checked before use.
    let raw =
        check(unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) })?;
    // SAFETY: `raw` is a new descriptor that nothing else owns.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    // SAFETY: an all-zero sockaddr_vm is a valid value.
    let mut addr: libc::sockaddr_vm = unsafe { mem::zeroed() };
    addr.svm_family = libc::AF_VSOCK as libc::sa_family_t;
    addr.svm_cid = cid;
    addr.svm_port = port;
    // SAFETY: `addr` is a live sockaddr_vm and the length passed is its size.
    check(unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&raw const addr).cast(),
            mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t,
        )
    })?;
    Ok(fd)
}

/// Asks the kernel to reseed its random generator now. Only the random device accepts it.
pub fn reseed_crng(fd: BorrowedFd) -> io::Result<()> {
    // SAFETY: this request takes no argument, so the kernel reads no memory of ours.
    check(unsafe { libc::ioctl(fd.as_raw_fd(), RNDRESEEDCRNG) }).map(drop)
}

fn secs(t: libc::timeval) -> f64 {
    t.tv_sec as f64 + t.tv_usec as f64 / 1e6
}

/// User plus system CPU seconds of all children this process has reaped so far.
pub fn children_cpu_seconds() -> io::Result<f64> {
    // SAFETY: an all-zero rusage is a valid value.
    let mut ru: libc::rusage = unsafe { mem::zeroed() };
    // SAFETY: `ru` is a live rusage that getrusage fills.
    check(unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, &mut ru) })?;
    Ok(secs(ru.ru_utime) + secs(ru.ru_stime))
}

/// How a child ended, worded as `std::process::ExitStatus` words it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    Code(i32),
    Signal(i32),
}

impl std::fmt::Display for Ended {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ended::Code(c) => write!(f, "exit status: {c}"),
            Ended::Signal(s) => write!(f, "signal: {s}"),
        }
    }
}

/// A reaped child: how it ended, and the user plus system CPU seconds it used, its threads and
/// the children it reaped included.
pub struct Reaped {
    pub ended: Ended,
    pub cpu_seconds: f64,
}

/// Waits for child `pid` to end and reaps it. `std::process::Child::wait` drops the resource
/// usage this reads.
pub fn wait4(pid: u32) -> io::Result<Reaped> {
    let mut status: libc::c_int = 0;
    // SAFETY: an all-zero `rusage` is a valid value for this plain C struct.
    let mut ru: libc::rusage = unsafe { mem::zeroed() };
    loop {
        // SAFETY: `status` and `ru` are live, writable locals of the types wait4 expects.
        let r = unsafe { libc::wait4(pid as libc::pid_t, &mut status, 0, &mut ru) };
        if r >= 0 {
            break;
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
    let ended = if libc::WIFEXITED(status) {
        Ended::Code(libc::WEXITSTATUS(status))
    } else {
        Ended::Signal(libc::WTERMSIG(status))
    };
    Ok(Reaped {
        ended,
        cpu_seconds: secs(ru.ru_utime) + secs(ru.ru_stime),
    })
}

/// The errno of `io_uring_setup`, or `None` when it succeeds (its descriptor is closed
/// again).
pub fn io_uring_setup_errno() -> Option<i32> {
    // The kernel's `struct io_uring_params` is 120 bytes, all-zero for a plain ring.
    let mut params = [0u64; 15];
    // SAFETY: io_uring_setup reads and writes only the 120-byte params struct, which
    // `params` is, and takes the entry count by value.
    let fd = unsafe { libc::syscall(libc::SYS_io_uring_setup, 1u32, params.as_mut_ptr()) };
    if fd < 0 {
        return io::Error::last_os_error().raw_os_error();
    }
    // SAFETY: the kernel just returned `fd`, which nothing else owns.
    unsafe { libc::close(fd as libc::c_int) };
    None
}

/// Sends SIGKILL to every process but this one. Only PID 1 may call it: there it reaches
/// every other process of the guest, and outside a guest it would reach far more.
pub fn kill_all_others() -> io::Result<()> {
    may_kill_all(std::process::id())?;
    // SAFETY: kill(2) takes no pointers.
    match check(unsafe { libc::kill(-1, libc::SIGKILL) }) {
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => Ok(()),
        r => r.map(drop),
    }
}

/// The guard of `kill_all_others`, apart so that a test never sends the kill.
fn may_kill_all(pid: u32) -> io::Result<()> {
    if pid == 1 {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "only PID 1 may kill all other processes",
    ))
}

#[cfg(test)]
mod tests {
    use super::{may_kill_all, wait4};

    #[test]
    fn only_pid_1_may_kill_all_others() {
        assert!(may_kill_all(1).is_ok());
        for pid in [0, 2, 1000] {
            let e = may_kill_all(pid).unwrap_err();
            assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
        }
    }

    #[test]
    fn wait4_of_a_pid_that_is_no_child_is_echild() {
        // A failed guard would retry for ever, so the call runs in a thread with a deadline.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(wait4(i32::MAX as u32).err()));
        let e = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("wait4 kept waiting")
            .expect("wait4 of a non-child succeeded");
        assert_eq!(e.raw_os_error(), Some(libc::ECHILD));
    }
}
