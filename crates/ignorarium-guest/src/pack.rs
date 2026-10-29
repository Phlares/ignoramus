//! Output pack: a strict byte stream for a directory tree. The guest writes it to a raw
//! disk; the host reads it back without parsing any guest file system.
//!
//! Format: `IGNPACK1`, then entries, then `e`. Each entry starts with a kind byte:
//! `d` dir, `f` file, `l` link, `o` other. Every kind but `e` is followed by the path
//! (u32 big-endian length, raw bytes, `/`-separated). `f` adds a u64 big-endian length
//! and the bytes. `l` adds a u32 big-endian length and the target bytes.
use crate::tree::{open_regular, walk};
use rustix::fs::{CWD, FileType, Mode, mknodat};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::Path;

const MAGIC: &[u8; 8] = b"IGNPACK1";
const MAX_PATH: usize = 4096;
const MAX_PART: usize = 255;
/// Charged to the limit for every entry: an entry costs the host an inode and a directory
/// block however few stream bytes it takes.
const ENTRY_COST: u64 = 4096;

struct Counter<'a, W> {
    inner: &'a mut W,
    n: u64,
}

impl<W: Write> Write for Counter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.n += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Writes `dir` as a pack and returns the number of bytes written.
pub fn write(dir: &Path, w: &mut impl Write) -> io::Result<u64> {
    let mut w = Counter { inner: w, n: 0 };
    w.write_all(MAGIC)?;
    walk(dir, &mut |rel, path, ty| {
        if ty.is_dir() {
            put_path(&mut w, b'd', rel)
        } else if ty.is_symlink() {
            put_path(&mut w, b'l', rel)?;
            put_bytes(&mut w, fs::read_link(path)?.as_os_str().as_bytes())
        } else if ty.is_file()
            && let Some(f) = open_regular(path)?
        {
            let len = f.metadata()?.len();
            put_path(&mut w, b'f', rel)?;
            w.write_all(&len.to_be_bytes())?;
            // A file that shrinks after the length is written is an error, not a short entry.
            if io::copy(&mut f.take(len), &mut w)? != len {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            Ok(())
        } else {
            put_path(&mut w, b'o', rel)
        }
    })?;
    w.write_all(b"e")?;
    Ok(w.n)
}

/// A u32 big-endian length, then the bytes.
fn put_bytes(w: &mut impl Write, b: &[u8]) -> io::Result<()> {
    w.write_all(&(b.len() as u32).to_be_bytes())?;
    w.write_all(b)
}

fn put_path(w: &mut impl Write, kind: u8, path: &[u8]) -> io::Result<()> {
    w.write_all(&[kind])?;
    put_bytes(w, path)
}

fn bad(msg: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Reads at most `left` more bytes; going past that is an error.
struct Reader<'a, R> {
    inner: &'a mut R,
    left: u64,
}

impl<R: Read> Reader<'_, R> {
    fn charge(&mut self, n: u64) -> io::Result<()> {
        if n > self.left {
            return Err(bad("pack is larger than the limit"));
        }
        self.left -= n;
        Ok(())
    }

    fn exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.charge(buf.len() as u64)?;
        self.inner.read_exact(buf).map_err(|e| match e.kind() {
            io::ErrorKind::UnexpectedEof => bad("truncated pack"),
            _ => e,
        })
    }

    fn u32(&mut self) -> io::Result<u32> {
        let mut b = [0; 4];
        self.exact(&mut b)?;
        Ok(u32::from_be_bytes(b))
    }

    fn u64(&mut self) -> io::Result<u64> {
        let mut b = [0; 8];
        self.exact(&mut b)?;
        Ok(u64::from_be_bytes(b))
    }

    /// A byte string of at most `MAX_PATH` bytes without NUL.
    fn name(&mut self, what: &str) -> io::Result<Vec<u8>> {
        let len = self.u32()? as usize;
        if len > MAX_PATH {
            return Err(bad(format!("{what} is longer than {MAX_PATH} bytes")));
        }
        let mut v = vec![0; len];
        self.exact(&mut v)?;
        if v.contains(&0) {
            return Err(bad(format!("{what} holds a NUL byte")));
        }
        Ok(v)
    }
}

fn check_path(path: &[u8]) -> Result<(), &'static str> {
    for part in path.split(|&b| b == b'/') {
        match part {
            p if p.len() > MAX_PART => return Err("path part is too long"),
            b"" => return Err("empty or absolute path part"),
            b"." | b".." => return Err("dot path part"),
            _ => {}
        }
    }
    Ok(())
}

/// Unpacks a pack into the empty directory `dest`. `limit` caps the bytes read plus a fixed
/// cost per entry.
/// Every entry is checked before anything is made for it; nothing is made outside `dest`.
pub fn unpack(r: &mut impl Read, dest: &Path, limit: u64) -> io::Result<()> {
    if fs::read_dir(dest)?.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "dest is not empty",
        ));
    }
    let mut r = Reader {
        inner: r,
        left: limit,
    };
    let mut magic = [0; 8];
    r.exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(bad("missing pack magic"));
    }
    // Each path so far, with its kind byte.
    let mut seen: HashMap<Vec<u8>, u8> = HashMap::new();
    loop {
        let mut kind = [0];
        r.exact(&mut kind)?;
        if kind[0] == b'e' {
            return Ok(());
        }
        if !matches!(kind[0], b'd' | b'f' | b'l' | b'o') {
            return Err(bad(format!("unknown entry kind {:#04x}", kind[0])));
        }
        r.charge(ENTRY_COST)?;
        let path = r.name("path")?;
        let shown = String::from_utf8_lossy(&path).into_owned();
        let entry = |why: &str| bad(format!("entry {shown:?}: {why}"));
        check_path(&path).map_err(entry)?;
        if let Some(at) = path.iter().rposition(|&b| b == b'/')
            && seen.get(&path[..at]) != Some(&b'd')
        {
            return Err(entry("parent is not an earlier directory"));
        }
        let to = dest.join(OsStr::from_bytes(&path));
        if seen.insert(path, kind[0]).is_some() {
            return Err(entry("duplicate path"));
        }
        if to.as_os_str().len() >= MAX_PATH {
            return Err(entry("path with the destination is too long"));
        }
        match kind[0] {
            b'd' => fs::create_dir(&to)?,
            b'f' => {
                let len = r.u64()?;
                r.charge(len).map_err(|_| entry("file is past the limit"))?;
                let mut f = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&to)?;
                if io::copy(&mut r.inner.by_ref().take(len), &mut f)? != len {
                    return Err(entry("file is past the end of the pack"));
                }
            }
            b'l' => {
                let target = r.name("link target").map_err(|e| match e.kind() {
                    io::ErrorKind::InvalidData => entry(&e.to_string()),
                    _ => e,
                })?;
                if target.is_empty() {
                    return Err(entry("empty link target"));
                }
                if target.len() >= MAX_PATH {
                    return Err(entry("link target is too long"));
                }
                symlink(OsStr::from_bytes(&target), &to)?;
            }
            _ => mknodat(CWD, &to, FileType::Fifo, Mode::RUSR | Mode::WUSR, 0)
                .map_err(io::Error::from)?,
        }
    }
}
