//! The check a level 1 sandbox runs on itself before each row: the network is cut off,
//! `AF_VSOCK` sockets are refused and `io_uring_setup` is refused. The box runs it as
//! `ignorarium dev selftest` inside a sandbox built like the row's.
use crate::sys;
use rustix::net::{AddressFamily, SocketType, socket};
use serde::{Deserialize, Serialize};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// An address reserved for documentation (RFC 5737): no route reaches it.
const PROBE_ADDR: &str = "192.0.2.1:9";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfTest {
    /// No interface but `lo`, and an outbound connect fails.
    pub connect_blocked: bool,
    pub vsock_blocked: bool,
    pub io_uring_blocked: bool,
}

impl SelfTest {
    pub fn ok(&self) -> bool {
        self.failed().is_empty()
    }

    /// The checks that did not hold, by short name.
    pub fn failed(&self) -> Vec<&'static str> {
        [
            (self.connect_blocked, "connect"),
            (self.vsock_blocked, "vsock"),
            (self.io_uring_blocked, "io_uring"),
        ]
        .into_iter()
        .filter_map(|(blocked, name)| (!blocked).then_some(name))
        .collect()
    }
}

/// Run all three checks in this process.
pub fn run() -> SelfTest {
    SelfTest {
        connect_blocked: connect_blocked(),
        vsock_blocked: vsock_blocked(),
        io_uring_blocked: sys::io_uring_setup_fails(),
    }
}

fn connect_blocked() -> bool {
    let only_lo = std::fs::read_to_string("/proc/net/dev").is_ok_and(|t| only_loopback(&t));
    let addr: SocketAddr = PROBE_ADDR.parse().expect("a fixed, valid address");
    only_lo && TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).is_err()
}

fn vsock_blocked() -> bool {
    let family = AddressFamily::from_raw(libc::AF_VSOCK as u16);
    // A socket that opens is closed again when the result drops.
    socket(family, SocketType::STREAM, None).is_err()
}

/// True when `/proc/net/dev` text lists no interface but `lo`.
fn only_loopback(dev: &str) -> bool {
    dev.lines()
        .skip(2)
        .filter_map(|l| l.split_once(':'))
        .all(|(name, _)| name.trim() == "lo")
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "Inter-|   Receive\n face |bytes packets\n";

    #[test]
    fn only_loopback_reads_interface_names() {
        assert!(only_loopback(&format!("{HEAD}    lo: 0 0\n")));
        assert!(only_loopback(HEAD));
        assert!(!only_loopback(&format!("{HEAD}    lo: 0 0\n  eth0: 5 1\n")));
        assert!(!only_loopback(&format!("{HEAD}  eth0: 5 1\n")));
        // The header lines carry colons or pipes of their own and never count.
        assert!(only_loopback("a: b\nc: d\n    lo: 1\n"));
    }

    #[test]
    fn failed_names_each_check_that_did_not_hold() {
        let all = SelfTest {
            connect_blocked: true,
            vsock_blocked: true,
            io_uring_blocked: true,
        };
        assert!(all.ok() && all.failed().is_empty());
        let none = SelfTest {
            connect_blocked: false,
            vsock_blocked: false,
            io_uring_blocked: false,
        };
        assert!(!none.ok());
        assert_eq!(none.failed(), ["connect", "vsock", "io_uring"]);
        for (t, name) in [
            (
                SelfTest {
                    connect_blocked: false,
                    ..all
                },
                "connect",
            ),
            (
                SelfTest {
                    vsock_blocked: false,
                    ..all
                },
                "vsock",
            ),
            (
                SelfTest {
                    io_uring_blocked: false,
                    ..all
                },
                "io_uring",
            ),
        ] {
            assert!(!t.ok());
            assert_eq!(t.failed(), [name]);
        }
    }

    #[test]
    fn the_json_line_names_the_three_fields() {
        let t = SelfTest {
            connect_blocked: true,
            vsock_blocked: false,
            io_uring_blocked: true,
        };
        assert_eq!(
            serde_json::to_string(&t).unwrap(),
            r#"{"connect_blocked":true,"vsock_blocked":false,"io_uring_blocked":true}"#
        );
    }
}
