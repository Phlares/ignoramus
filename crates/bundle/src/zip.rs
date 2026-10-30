//! Deterministic stored zip: same files, same bytes. No times, modes or extras.
use crate::manifest::each_chunk;
use crate::tree::{self, Found};
use crate::{Error, Problem, refuse_any};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

const INSTRUCTION: &str = "Deposit files of 4 GiB or more as artifacts beside the bundle; a bundle zip holds at most 65,534 files under 4 GiB in all.";
const LIMIT: u64 = 0xFFFF_FFFF;
// A count of 0xFFFF in the end record means "read the Zip64 record", which this zip has not.
const MAX_FILES: usize = 65_534;
const DATE: u16 = 0x0021;
const FLAGS: u16 = 0x0800;

const fn table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 == 1 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static TABLE: [u32; 256] = table();

fn crc_update(crc: u32, bytes: &[u8]) -> u32 {
    bytes.iter().fold(crc, |c, &b| {
        TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8)
    })
}

pub fn crc32(bytes: &[u8]) -> u32 {
    !crc_update(!0, bytes)
}

fn limit(path: &str, message: &str) -> Problem {
    Problem::new("E-PUB-012", path, message, INSTRUCTION)
}

/// Checks the limits from the walk's sizes only. No file is read.
fn check_limits(files: &[Found]) -> Result<(), Error> {
    let mut problems = Vec::new();
    if files.len() > MAX_FILES {
        problems.push(limit("", "too many files for a bundle zip"));
    }
    // End record, plus per file: local header 30, central entry 46, name twice, data.
    let mut total: u64 = 22;
    for f in files {
        if f.len >= LIMIT {
            problems.push(limit(&tree::shown(&f.path), "too large for a bundle zip"));
        } else if f.path.len() > usize::from(u16::MAX) {
            problems.push(limit(
                &tree::shown(&f.path),
                "name too long for a bundle zip",
            ));
        } else {
            total += 76 + 2 * f.path.len() as u64 + f.len;
        }
    }
    if total >= LIMIT {
        problems.push(limit("", "bundle zip would reach 4 GiB"));
    }
    refuse_any(problems)
}

fn crc_reader(reader: &mut impl Read) -> io::Result<(u32, u64)> {
    let mut crc = !0u32;
    let size = each_chunk(reader, |b| {
        crc = crc_update(crc, b);
        Ok(())
    })?;
    Ok((!crc, size))
}

/// Counts and hashes every byte written through it.
struct Tee<W: Write> {
    inner: W,
    hasher: Sha256,
    written: u64,
}

impl<W: Write> Write for Tee<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.written += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn put16(w: &mut impl Write, values: &[u16]) -> io::Result<()> {
    values
        .iter()
        .try_for_each(|v| w.write_all(&v.to_le_bytes()))
}

fn put32(w: &mut impl Write, v: u32) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

fn too_big<T>(r: Result<T, std::num::TryFromIntError>) -> io::Result<T> {
    r.map_err(|_| io::Error::other("bundle zip field out of range"))
}

/// Writes `dir` as a stored zip at `out`, which must not exist.
/// Returns the SHA-256 of the zip bytes.
pub fn pack(dir: &Path, out: &Path) -> Result<String, Error> {
    let (files, problems) = tree::walk(dir)?;
    refuse_any(problems)?;
    pack_files(dir, &files, out)
}

/// Like `pack`, for files a walk of `dir` already found. Each file is opened once
/// through `tree::open`, so a swap after the walk is refused.
pub fn pack_files(dir: &Path, files: &[Found], out: &Path) -> Result<String, Error> {
    check_limits(files)?;
    let file = File::options().write(true).create_new(true).open(out)?;
    let mut w = Tee {
        inner: BufWriter::new(file),
        hasher: Sha256::new(),
        written: 0,
    };
    let mut central = Vec::new();
    for f in files {
        let mut src = tree::open(dir, f)?;
        let (crc, counted) = crc_reader(&mut src)?;
        if counted != f.len {
            return Err(tree::changed(&f.path).into());
        }
        src.seek(SeekFrom::Start(0))?;
        let size = too_big(u32::try_from(f.len))?;
        let bytes = f.path.as_bytes();
        let len = too_big(u16::try_from(bytes.len()))?;
        let offset = too_big(u32::try_from(w.written))?;

        put32(&mut w, 0x0403_4b50)?;
        put16(&mut w, &[20, FLAGS, 0, 0, DATE])?;
        put32(&mut w, crc)?;
        put32(&mut w, size)?;
        put32(&mut w, size)?;
        put16(&mut w, &[len, 0])?;
        w.write_all(bytes)?;
        let copied = io::copy(&mut src.take(f.len), &mut w)?;
        if copied != f.len {
            return Err(tree::changed(&f.path).into());
        }

        put32(&mut central, 0x0201_4b50)?;
        put16(&mut central, &[0x0314, 20, FLAGS, 0, 0, DATE])?;
        put32(&mut central, crc)?;
        put32(&mut central, size)?;
        put32(&mut central, size)?;
        put16(&mut central, &[len, 0, 0, 0, 0])?;
        put32(&mut central, 0x81A4_0000)?;
        put32(&mut central, offset)?;
        central.write_all(bytes)?;
    }
    let central_offset = too_big(u32::try_from(w.written))?;
    let count = too_big(u16::try_from(files.len()))?;
    let central_size = too_big(u32::try_from(central.len()))?;
    w.write_all(&central)?;
    put32(&mut w, 0x0605_4b50)?;
    put16(&mut w, &[0, 0, count, count])?;
    put32(&mut w, central_size)?;
    put32(&mut w, central_offset)?;
    put16(&mut w, &[0])?;
    w.flush()?;
    Ok(hex::encode(w.hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(files: &[Found]) -> Vec<(String, String)> {
        match check_limits(files) {
            Ok(()) => Vec::new(),
            Err(Error::Refused { problems, .. }) => {
                problems.into_iter().map(|p| (p.path, p.message)).collect()
            }
            Err(e) => panic!("{e}"),
        }
    }

    fn one(path: &str, message: &str) -> Vec<(String, String)> {
        vec![(path.to_string(), message.to_string())]
    }

    #[test]
    fn a_zip_holds_at_most_65534_files() {
        let files = |n| vec![Found::fake("a", 0); n];
        assert_eq!(refused(&files(65_534)), []);
        assert_eq!(
            refused(&files(65_535)),
            one("", "too many files for a bundle zip")
        );
    }

    #[test]
    fn a_name_holds_at_most_65535_bytes() {
        let named = |n| Found::fake(&"a".repeat(n), 0);
        assert_eq!(refused(&[named(65_535)]), []);
        assert_eq!(
            refused(&[named(65_536)]),
            one(&"a".repeat(65_536), "name too long for a bundle zip")
        );
    }

    #[test]
    fn the_zip_stays_below_4_gib() {
        // The end record (22), both headers (76) and the name twice (6) take 104 bytes.
        assert_eq!(refused(&[Found::fake("abc", LIMIT - 105)]), []);
        assert_eq!(
            refused(&[Found::fake("abc", LIMIT - 104)]),
            one("", "bundle zip would reach 4 GiB")
        );
        assert_eq!(
            refused(&[Found::fake("abc", LIMIT)]),
            one("abc", "too large for a bundle zip")
        );
    }

    struct FailingFlush;

    impl Write for FailingFlush {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("disk full"))
        }
    }

    #[test]
    fn tee_passes_a_flush_error_on() {
        let mut t = Tee {
            inner: FailingFlush,
            hasher: Sha256::new(),
            written: 0,
        };
        t.write_all(b"abc").unwrap();
        assert_eq!(t.written, 3);
        assert_eq!(t.flush().unwrap_err().to_string(), "disk full");
    }
}
