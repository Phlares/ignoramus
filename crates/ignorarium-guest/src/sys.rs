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

/// User plus system CPU seconds of all children this process has reaped so far.
pub fn children_cpu_seconds() -> io::Result<f64> {
    // SAFETY: an all-zero rusage is a valid value.
    let mut ru: libc::rusage = unsafe { mem::zeroed() };
    // SAFETY: `ru` is a live rusage that getrusage fills.
    check(unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, &mut ru) })?;
    let secs = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    Ok(secs(ru.ru_utime) + secs(ru.ru_stime))
}

/// Sends SIGKILL to every process but this one. Only PID 1 may call it: there it reaches
/// every other process of the guest, and outside a guest it would reach far more.
pub fn kill_all_others() -> io::Result<()> {
    if std::process::id() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "only PID 1 may kill all other processes",
        ));
    }
    // SAFETY: kill(2) takes no pointers.
    match check(unsafe { libc::kill(-1, libc::SIGKILL) }) {
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => Ok(()),
        r => r.map(drop),
    }
}
