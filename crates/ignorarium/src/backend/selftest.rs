//! The check a level 1 sandbox runs on itself before each row: the network is cut off,
//! `AF_VSOCK` sockets are refused and `io_uring_setup` is refused. The box runs it as
//! `ignorarium dev selftest` inside a sandbox built like the row's.
use ignorarium_guest::sys;
use rustix::net::{AddressFamily, SocketType, socket};
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

/// An address reserved for documentation (RFC 5737, TEST-NET-1): no route reaches it.
const PROBE_ADDR: (Ipv4Addr, u16) = (Ipv4Addr::new(192, 0, 2, 1), 9);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfTest {
    /// No interface but `lo`, and an outbound connect fails.
    pub connect_blocked: bool,
    pub vsock_blocked: bool,
    pub io_uring_blocked: bool,
    /// The call is blocked because the kernel lacks it (`ENOSYS`, `EAFNOSUPPORT`), not
    /// because the filter denied it.
    #[serde(default)]
    pub vsock_absent: bool,
    #[serde(default)]
    pub io_uring_absent: bool,
}

/// What one call did: `EPERM` is the filter's answer; a missing feature counts as blocked
/// but is reported as absent; success or any other errno leaves the call open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Call {
    Denied,
    Absent,
    Open,
}

impl Call {
    pub fn of(errno: Option<i32>) -> Call {
        match errno {
            Some(libc::EPERM) => Call::Denied,
            Some(libc::ENOSYS | libc::EAFNOSUPPORT) => Call::Absent,
            _ => Call::Open,
        }
    }
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

    /// `denied`, `absent` or `open`.
    pub fn vsock_state(&self) -> &'static str {
        state(self.vsock_blocked, self.vsock_absent)
    }

    pub fn io_uring_state(&self) -> &'static str {
        state(self.io_uring_blocked, self.io_uring_absent)
    }
}

fn state(blocked: bool, absent: bool) -> &'static str {
    match (blocked, absent) {
        (false, _) => "open",
        (true, true) => "absent",
        (true, false) => "denied",
    }
}

/// Run all three checks in this process.
pub fn run() -> SelfTest {
    let vsock = Call::of(vsock_errno());
    let io_uring = Call::of(sys::io_uring_setup_errno());
    let addr = SocketAddr::from(PROBE_ADDR);
    let connect_blocked =
        std::fs::read_to_string("/proc/net/dev").is_ok_and(|dev| connect_blocked(&dev, addr));
    SelfTest {
        connect_blocked,
        vsock_blocked: vsock != Call::Open,
        io_uring_blocked: io_uring != Call::Open,
        vsock_absent: vsock == Call::Absent,
        io_uring_absent: io_uring == Call::Absent,
    }
}

/// The errno of an `AF_VSOCK` socket, or `None` when one opens (and closes again).
fn vsock_errno() -> Option<i32> {
    let family = AddressFamily::from_raw(libc::AF_VSOCK as u16);
    socket(family, SocketType::STREAM, None)
        .err()
        .map(|e| e.raw_os_error())
}

/// True when `dev` (the text of `/proc/net/dev`) lists no interface but `lo`, and a
/// connect to `addr` fails.
fn connect_blocked(dev: &str, addr: SocketAddr) -> bool {
    only_loopback(dev) && TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).is_err()
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
    use std::net::TcpListener;

    const HEAD: &str = "Inter-|   Receive\n face |bytes packets\n";

    fn lo_only() -> String {
        format!("{HEAD}    lo: 0 0\n")
    }

    #[test]
    fn only_loopback_reads_interface_names() {
        assert!(only_loopback(&lo_only()));
        assert!(only_loopback(HEAD));
        assert!(!only_loopback(&format!("{HEAD}    lo: 0 0\n  eth0: 5 1\n")));
        assert!(!only_loopback(&format!("{HEAD}  eth0: 5 1\n")));
        // The header lines carry colons or pipes of their own and never count.
        assert!(only_loopback("a: b\nc: d\n    lo: 1\n"));
    }

    #[test]
    fn connect_blocked_needs_both_no_interface_and_a_failed_connect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let open = listener.local_addr().unwrap();
        // A port nothing listens on: the connect is refused at once.
        let closed = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        let eth = format!("{HEAD}  eth0: 5 1\n");
        assert!(connect_blocked(&lo_only(), closed));
        assert!(!connect_blocked(&lo_only(), open));
        assert!(!connect_blocked(&eth, closed));
        assert!(!connect_blocked(&eth, open));
    }

    #[test]
    fn a_call_is_denied_absent_or_open_by_its_errno() {
        assert_eq!(Call::of(Some(libc::EPERM)), Call::Denied);
        assert_eq!(Call::of(Some(libc::ENOSYS)), Call::Absent);
        assert_eq!(Call::of(Some(libc::EAFNOSUPPORT)), Call::Absent);
        assert_eq!(Call::of(Some(libc::EACCES)), Call::Open);
        assert_eq!(Call::of(Some(libc::EINVAL)), Call::Open);
        assert_eq!(Call::of(None), Call::Open);
    }

    fn all() -> SelfTest {
        SelfTest {
            connect_blocked: true,
            vsock_blocked: true,
            io_uring_blocked: true,
            vsock_absent: false,
            io_uring_absent: false,
        }
    }

    #[test]
    fn failed_names_each_check_that_did_not_hold() {
        assert!(all().ok() && all().failed().is_empty());
        let none = SelfTest {
            connect_blocked: false,
            vsock_blocked: false,
            io_uring_blocked: false,
            ..all()
        };
        assert!(!none.ok());
        assert_eq!(none.failed(), ["connect", "vsock", "io_uring"]);
        for (t, name) in [
            (
                SelfTest {
                    connect_blocked: false,
                    ..all()
                },
                "connect",
            ),
            (
                SelfTest {
                    vsock_blocked: false,
                    ..all()
                },
                "vsock",
            ),
            (
                SelfTest {
                    io_uring_blocked: false,
                    ..all()
                },
                "io_uring",
            ),
        ] {
            assert!(!t.ok());
            assert_eq!(t.failed(), [name]);
        }
    }

    #[test]
    fn states_tell_denied_from_absent_from_open() {
        assert_eq!(all().vsock_state(), "denied");
        assert_eq!(all().io_uring_state(), "denied");
        let absent = SelfTest {
            vsock_absent: true,
            io_uring_absent: true,
            ..all()
        };
        assert_eq!(absent.vsock_state(), "absent");
        assert_eq!(absent.io_uring_state(), "absent");
        let open = SelfTest {
            vsock_blocked: false,
            io_uring_blocked: false,
            ..all()
        };
        assert_eq!(open.vsock_state(), "open");
        assert_eq!(open.io_uring_state(), "open");
    }

    #[test]
    fn the_json_line_names_the_fields_and_reads_the_short_form() {
        let t = SelfTest {
            vsock_blocked: false,
            io_uring_absent: true,
            ..all()
        };
        assert_eq!(
            serde_json::to_string(&t).unwrap(),
            r#"{"connect_blocked":true,"vsock_blocked":false,"io_uring_blocked":true,"vsock_absent":false,"io_uring_absent":true}"#
        );
        let short: SelfTest = serde_json::from_str(
            r#"{"connect_blocked":true,"vsock_blocked":true,"io_uring_blocked":true}"#,
        )
        .unwrap();
        assert_eq!(short, all());
    }
}
