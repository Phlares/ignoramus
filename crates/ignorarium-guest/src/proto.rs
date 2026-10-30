//! Control frames: a 4-byte big-endian length, then that many bytes of JSON.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io::{self, Read, Write};

pub const CONTROL_PORT: u32 = 7201;
pub const HOST_CID: u32 = 2;
pub const GUEST_CID: u32 = 3;
pub const MAX_FRAME: usize = 1 << 20;
/// Where the payload sees the input and output trees, and its own folder.
pub const GUEST_IN: &str = "/in";
pub const GUEST_OUT: &str = "/out";
pub const GUEST_PAYLOAD: &str = "/payload";
pub const SEED_ENV: &str = "IGNORARIUM_SEED";
pub const SERIAL_ROOTFS: &str = "ign-rootfs";
pub const SERIAL_IN: &str = "ign-in";
pub const SERIAL_OUT: &str = "ign-out";
pub const SERIAL_PAYLOAD: &str = "ign-payload";

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Job {
    pub seed_hex: String,
    pub unix_nanos: u64,
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub timeout_ms: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SelfTest {
    pub only_loopback: bool,
    pub route_blocked: bool,
    pub host_path_blocked: bool,
    /// The errno of `io_uring_setup`; `None` when the call worked.
    pub io_uring_errno: Option<i32>,
}

impl SelfTest {
    pub fn ok(&self) -> bool {
        self.only_loopback
            && self.route_blocked
            && self.host_path_blocked
            && self.io_uring_blocked()
    }

    /// `EPERM` is a refusal; `ENOSYS` and `EAFNOSUPPORT` mean the kernel lacks the call. Any
    /// other errno leaves it open. The box reads a level 1 sandbox's call the same way.
    fn io_uring_blocked(&self) -> bool {
        matches!(
            self.io_uring_errno,
            Some(libc::EPERM | libc::ENOSYS | libc::EAFNOSUPPORT)
        )
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum End {
    Code(i32),
    Signal(i32),
    Timeout,
}

/// `end: None` means the payload did not run.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Report {
    pub selftest: SelfTest,
    pub clock_unix_nanos: u64,
    pub end: Option<End>,
    pub payload_wall_ns: u64,
    pub cpu_seconds: f64,
    pub output_hash: String,
    /// Why the pack refused what the payload left in `/out`: the payload's own result, so
    /// no outputs come back and `output_hash` is empty.
    pub pack_refused: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToHost {
    Hello { agent: String },
    Report(Report),
    Failed { reason: String },
}

pub fn write_frame<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    if body.len() > MAX_FRAME {
        return Err(too_big(body.len()));
    }
    w.write_all(&(body.len() as u32).to_be_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

pub fn read_frame<T: DeserializeOwned>(r: &mut impl Read) -> io::Result<T> {
    let mut head = [0u8; 4];
    r.read_exact(&mut head)?;
    let n = u32::from_be_bytes(head) as usize;
    if n > MAX_FRAME {
        return Err(too_big(n));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)?;
    // The whole body is here, so JSON that ends early is bad data, not a cut stream.
    serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn too_big(n: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("frame of {n} bytes exceeds the {MAX_FRAME}-byte limit"),
    )
}

pub fn seed_from_hex(s: &str) -> io::Result<[u8; 32]> {
    let bad = |m: String| io::Error::new(io::ErrorKind::InvalidData, m);
    if s.len() != 64 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(bad("seed must be 64 lowercase hex digits".into()));
    }
    let mut out = [0u8; 32];
    hex::decode_to_slice(s, &mut out).map_err(|e| bad(e.to_string()))?;
    Ok(out)
}
