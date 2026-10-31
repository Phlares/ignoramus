use ignorarium_guest::proto::*;
use std::io::{Cursor, ErrorKind, Read};

fn job() -> Job {
    Job {
        seed_hex: "ab".repeat(32),
        unix_nanos: 1_700_000_000_123_456_789,
        argv: vec!["/bin/x".into(), "--y".into()],
        env: vec![("A".into(), "b".into())],
        timeout_ms: 5000,
    }
}

fn report() -> Report {
    Report {
        selftest: SelfTest {
            only_loopback: true,
            route_blocked: true,
            host_path_blocked: false,
            io_uring_errno: Some(1),
        },
        clock_unix_nanos: 42,
        end: Some(End::Signal(9)),
        payload_wall_ns: 7,
        cpu_seconds: 0.25,
        output_hash: "00".repeat(32),
        pack_refused: Some("too big".into()),
    }
}

fn round<T: serde::Serialize + serde::de::DeserializeOwned>(v: &T) -> T {
    let mut buf = Vec::new();
    write_frame(&mut buf, v).unwrap();
    read_frame(&mut Cursor::new(buf)).unwrap()
}

#[test]
fn frames_round_trip() {
    assert_eq!(round(&job()), job());
    let msgs = [
        ToHost::Hello {
            agent: "a 1".into(),
        },
        ToHost::Report(report()),
        ToHost::Report(Report {
            end: None,
            ..report()
        }),
        ToHost::Failed {
            reason: "no".into(),
        },
    ];
    for m in msgs {
        assert_eq!(round(&m), m);
    }
    for e in [End::Code(-1), End::Timeout] {
        assert_eq!(round(&e), e);
    }
}

#[test]
fn a_frame_is_a_big_endian_length_then_json() {
    let mut buf = Vec::new();
    write_frame(&mut buf, &ToHost::Hello { agent: "a".into() }).unwrap();
    let n = u32::from_be_bytes(buf[0..4].try_into().unwrap()) as usize;
    assert_eq!(n, buf.len() - 4);
    let v: serde_json::Value = serde_json::from_slice(&buf[4..]).unwrap();
    assert_eq!(v["type"], "hello");
}

struct Counting(Vec<u8>, usize);
impl Read for Counting {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = out.len().min(self.0.len() - self.1);
        out[..n].copy_from_slice(&self.0[self.1..self.1 + n]);
        self.1 += n;
        Ok(n)
    }
}

#[test]
fn an_oversized_frame_is_refused() {
    let mut r = Counting((MAX_FRAME as u32 + 1).to_be_bytes().to_vec(), 0);
    let e = read_frame::<Job>(&mut r).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::InvalidData);
    let msg = e.to_string();
    assert!(msg.contains("1048577") && msg.contains("1048576"), "{msg}");
    assert_eq!(r.1, 4, "read past the header");
}

#[test]
fn a_cut_frame_is_unexpected_eof() {
    let mut bytes = 10u32.to_be_bytes().to_vec();
    bytes.extend_from_slice(b"abcd");
    let e = read_frame::<Job>(&mut Cursor::new(bytes)).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::UnexpectedEof);
}

#[test]
fn a_truncated_json_body_in_a_whole_frame_is_invalid_data() {
    // The frame is complete; only its JSON ends early, so this is not a cut stream.
    let mut bytes = 5u32.to_be_bytes().to_vec();
    bytes.extend_from_slice(br#"{"a":"#);
    let e = read_frame::<serde_json::Value>(&mut Cursor::new(bytes)).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::InvalidData);
}

#[test]
fn bad_json_is_invalid_data() {
    let mut bytes = 4u32.to_be_bytes().to_vec();
    bytes.extend_from_slice(b"nope");
    let e = read_frame::<Job>(&mut Cursor::new(bytes)).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::InvalidData);
}

#[test]
fn seed_hex_must_be_64_lowercase_hex() {
    assert_eq!(seed_from_hex(&"0f".repeat(32)).unwrap(), [0x0f; 32]);
    for bad in [
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
    ] {
        assert!(seed_from_hex(&bad).is_err(), "{bad}");
    }
}

#[test]
fn selftest_ok_needs_every_check() {
    let t = SelfTest {
        only_loopback: true,
        route_blocked: true,
        host_path_blocked: true,
        io_uring_errno: Some(libc::EPERM),
    };
    assert!(t.ok());
    assert!(
        !SelfTest {
            only_loopback: false,
            ..t.clone()
        }
        .ok()
    );
    assert!(
        !SelfTest {
            route_blocked: false,
            ..t.clone()
        }
        .ok()
    );
    assert!(
        !SelfTest {
            host_path_blocked: false,
            ..t
        }
        .ok()
    );
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

/// A message whose JSON body is exactly `n` bytes.
fn failed_of_len(n: usize) -> ToHost {
    let bare = serde_json::to_vec(&ToHost::Failed { reason: "".into() })
        .unwrap()
        .len();
    ToHost::Failed {
        reason: "a".repeat(n - bare),
    }
}

#[test]
fn a_frame_of_exactly_the_limit_is_written_and_read() {
    let msg = failed_of_len(MAX_FRAME);
    let mut buf = Vec::new();
    write_frame(&mut buf, &msg).unwrap();
    assert_eq!(buf.len(), 4 + MAX_FRAME);
    assert_eq!(read_frame::<ToHost>(&mut Cursor::new(buf)).unwrap(), msg);
}

#[test]
fn a_frame_one_byte_over_the_limit_is_refused_by_write() {
    let mut buf = Vec::new();
    let e = write_frame(&mut buf, &failed_of_len(MAX_FRAME + 1)).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::InvalidData);
    assert!(e.to_string().contains("1048577"), "{e}");
    assert!(buf.is_empty(), "bytes written before the refusal");
}
