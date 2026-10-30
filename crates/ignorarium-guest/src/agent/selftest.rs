//! Checks that the guest has no way out: loopback only, no route, no path to the host, and
//! no io_uring.
use crate::proto::SelfTest;
use crate::sys;
use std::fs;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::path::Path;

pub fn run(net_dir: &Path, host_connect: impl Fn() -> io::Result<()>) -> SelfTest {
    SelfTest {
        only_loopback: only_loopback(net_dir),
        route_blocked: route_blocked(),
        host_path_blocked: host_connect().is_err(),
        io_uring_errno: sys::io_uring_setup_errno(),
    }
}

fn only_loopback(net_dir: &Path) -> bool {
    let Ok(dir) = fs::read_dir(net_dir) else {
        return false;
    };
    let names: Result<Vec<_>, _> = dir.map(|e| e.map(|e| e.file_name())).collect();
    matches!(names, Ok(n) if n.len() == 1 && n[0] == "lo")
}

/// True when UDP `connect` to a documentation address fails for IPv4 and IPv6.
/// UDP `connect` only looks up a route and sends nothing.
pub fn route_blocked() -> bool {
    let v4 = SocketAddr::from((Ipv4Addr::new(192, 0, 2, 1), 9));
    let v6 = SocketAddr::from((Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1), 9));
    blocked((Ipv4Addr::UNSPECIFIED, 0).into(), v4) && blocked((Ipv6Addr::UNSPECIFIED, 0).into(), v6)
}

fn blocked(local: SocketAddr, remote: SocketAddr) -> bool {
    match UdpSocket::bind(local) {
        // The guest kernel has no IPv6, so that family cannot be used at all.
        Err(e) => e.raw_os_error() == Some(libc::EAFNOSUPPORT),
        Ok(s) => s.connect(remote).is_err(),
    }
}

#[cfg(test)]
mod tests {
    use super::blocked;
    use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

    #[test]
    fn a_bind_that_fails_for_another_reason_is_not_blocked() {
        // A documentation address that no host owns: bind fails with EADDRNOTAVAIL.
        let local = SocketAddr::from((Ipv4Addr::new(192, 0, 2, 77), 0));
        let remote = SocketAddr::from((Ipv4Addr::new(192, 0, 2, 1), 9));
        let err = UdpSocket::bind(local).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EADDRNOTAVAIL));
        assert!(!blocked(local, remote));
    }
}
