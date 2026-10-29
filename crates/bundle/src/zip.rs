//! Deterministic stored zip: same files, same bytes. No times, modes or extras.
use crate::tree::{self, Found};
use crate::{Error, Problem};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

const INSTRUCTION: &str = "Deposit files of 4 GiB or more as artifacts beside the bundle; a bundle zip holds at most 65,535 files under 4 GiB in all.";
const LIMIT: u64 = 0xFFFF_FFFF;
const MAX_FILES: usize = 65_535;
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
    Problem {
        code: "E-PUB-012",
        path: path.to_string(),
        message: message.to_string(),
        instruction: INSTRUCTION.to_string(),
    }
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
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Error::Refused {
            problems,
            warnings: Vec::new(),
        })
    }
}

fn crc_reader(reader: &mut impl Read) -> io::Result<(u32, u64)> {
    let mut buf = vec![0u8; 64 * 1024];
    let (mut crc, mut size) = (!0u32, 0u64);
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok((!crc, size));
        }
        crc = crc_update(crc, &buf[..n]);
        size += n as u64;
    }
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

fn changed(name: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{} changed while the bundle was read", tree::shown(name)),
    )
}

/// Writes `dir` as a stored zip at `out`, which must not exist.
/// Returns the SHA-256 of the zip bytes.
pub fn pack(dir: &Path, out: &Path) -> Result<String, Error> {
    let (files, problems) = tree::walk(dir)?;
    if !problems.is_empty() {
        return Err(Error::Refused {
            problems,
            warnings: Vec::new(),
        });
    }
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
            return Err(changed(&f.path).into());
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
            return Err(changed(&f.path).into());
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
