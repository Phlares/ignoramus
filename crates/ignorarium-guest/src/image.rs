//! Host-side parts of a guest image: pinned downloads, the initramfs and the ext4 disks.
use crate::tree::{sha256_hex, walk};
use std::fs::{self, File};
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// A download pinned by content hash.
pub struct Pin {
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
}

pub const KERNEL: Pin = Pin {
    name: "vmlinux-x86_64 ch-release-v7.2.8-20261009",
    url: "https://github.com/cloud-hypervisor/linux/releases/download/ch-release-v7.2.8-20261009/vmlinux-x86_64",
    sha256: "edfeb8a8a6c04ff2979e9f1c700feaa29b8f564f952261834ef0b7d2e673a853",
};

pub const VMM: Pin = Pin {
    name: "cloud-hypervisor-static v54.0",
    url: "https://github.com/cloud-hypervisor/cloud-hypervisor/releases/download/v54.0/cloud-hypervisor-static",
    sha256: "4f4f98a3d661a40f3e5272627cc895a1b2e525f2f6bedf5f895f9cd21ba5c65b",
};

pub fn sha256_file(path: &Path) -> io::Result<String> {
    sha256_hex(File::open(path)?)
}

pub fn check_pin(path: &Path, pin: &Pin) -> io::Result<()> {
    let actual = sha256_file(path)?;
    if actual == pin.sha256 {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "{} at {}: expected sha256 {}, found {}",
            pin.name,
            path.display(),
            pin.sha256,
            actual
        ),
    ))
}

/// True when the 64-bit little-endian ELF file `elf` names a program interpreter, so it
/// needs a dynamic loader. `InvalidData` for anything else, a cut file included. Layout:
/// <https://man7.org/linux/man-pages/man5/elf.5.html>
pub fn has_interp(elf: &[u8]) -> io::Result<bool> {
    const PT_INTERP: u64 = 3;
    let bad = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "not a 64-bit little-endian ELF file",
        )
    };
    if !elf.starts_with(b"\x7fELF\x02\x01") {
        return Err(bad());
    }
    // A field of `n` bytes (at most 8), little-endian.
    let le = |at: u64, n: usize| -> io::Result<u64> {
        let start = usize::try_from(at).map_err(|_| bad())?;
        let end = start.checked_add(n).ok_or_else(bad)?;
        let mut field = [0u8; 8];
        field[..n].copy_from_slice(elf.get(start..end).ok_or_else(bad)?);
        Ok(u64::from_le_bytes(field))
    };
    let (phoff, size, count) = (le(0x20, 8)?, le(0x36, 2)?, le(0x38, 2)?);
    for i in 0..count {
        let at = i.checked_mul(size).and_then(|o| o.checked_add(phoff));
        if le(at.ok_or_else(bad)?, 4)? == PT_INTERP {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Appends one entry of the "newc" cpio format:
/// <https://docs.kernel.org/driver-api/early-userspace/buffer-format.html>
fn newc_entry(out: &mut Vec<u8>, ino: u32, name: &str, mode: u32, rdev: (u32, u32), data: &[u8]) {
    let nlink = if mode & 0o170000 == 0o040000 { 2 } else { 1 };
    let name_size = name.len() as u32 + 1;
    let fields = [
        ino,
        mode,
        0,
        0,
        nlink,
        0,
        data.len() as u32,
        0,
        0,
        rdev.0,
        rdev.1,
        name_size,
        0,
    ];
    out.extend_from_slice(b"070701");
    for f in fields {
        out.extend_from_slice(format!("{f:08X}").as_bytes());
    }
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    out.resize(out.len().next_multiple_of(4), 0);
    out.extend_from_slice(data);
    out.resize(out.len().next_multiple_of(4), 0);
}

/// A boot archive holding `/dev/console`, `/dev/null` and the agent as `/init`. When PID 1
/// starts with any of fds 0-2 closed, std opens `/dev/null` there, or aborts if it cannot.
pub fn initramfs(agent: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    newc_entry(&mut out, 1, "dev", 0o040755, (0, 0), &[]);
    newc_entry(&mut out, 2, "dev/console", 0o020600, (5, 1), &[]);
    newc_entry(&mut out, 3, "dev/null", 0o020666, (1, 3), &[]);
    newc_entry(&mut out, 4, "init", 0o100755, (0, 0), agent);
    newc_entry(&mut out, 5, "TRAILER!!!", 0, (0, 0), &[]);
    out
}

const BLOCK: u64 = 4096;

fn tree_bytes(dir: &Path) -> io::Result<u64> {
    let mut sum = 0;
    walk(dir, &mut |_, path, _| {
        sum += fs::symlink_metadata(path)?.len().next_multiple_of(BLOCK) + BLOCK;
        Ok(())
    })?;
    Ok(sum)
}

/// Image size for `dir`: each entry's size rounded up to a block, a block per entry
/// for inodes and directory slots, 25% slack, plus 16 MiB.
pub fn size_mib(dir: &Path) -> io::Result<u64> {
    Ok((tree_bytes(dir)? * 5 / 4).div_ceil(1 << 20) + 16)
}

/// Runs `cmd` to its end and returns what it wrote on stderr.
fn run(cmd: &mut Command) -> io::Result<String> {
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .map_err(|e| io::Error::new(e.kind(), format!("{:?}: {e}", cmd.get_program())))?;
    let err = String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        return Ok(err.into_owned());
    }
    let lines: Vec<&str> = err.lines().collect();
    let tail = lines[lines.len().saturating_sub(10)..].join("\n");
    Err(io::Error::other(format!(
        "{:?} failed ({}): {tail}",
        cmd.get_program(),
        out.status
    )))
}

/// The end of a debugfs request that wrote `stderr`. debugfs exits 0 when its request fails,
/// so any stderr line but its version banner is the failure.
fn debugfs_end(stderr: &str) -> io::Result<()> {
    match stderr.lines().find(|l| !l.starts_with("debugfs ")) {
        None => Ok(()),
        Some(why) => Err(io::Error::other(format!("debugfs failed: {why}"))),
    }
}

/// Makes an ext4 image of `size_mib` MiB at `dest`, filled from `src`. It has no journal,
/// so reading it on the host never needs a replay. Its root holds what `src` holds and no
/// more: mke2fs always makes `lost+found` (mode 0700, owned by root), which the guest's user
/// could not read, so it goes unless `src` has its own.
pub fn ext4(src: Option<&Path>, dest: &Path, size_mib: u64) -> io::Result<()> {
    File::create(dest)?.set_len(size_mib << 20)?;
    let mut cmd = Command::new("mkfs.ext4");
    // One inode per block: the size formula gives every entry at least a block.
    cmd.args([
        "-q",
        "-F",
        "-i",
        "4096",
        "-O",
        "^has_journal",
        "-E",
        "root_owner=0:0",
    ]);
    if let Some(src) = src {
        cmd.arg("-d").arg(src);
    }
    run(cmd.arg(dest))?;
    if src.is_some_and(|s| fs::symlink_metadata(s.join(LOST_FOUND)).is_ok()) {
        return Ok(());
    }
    let mut rmdir = Command::new("debugfs");
    rmdir.args(["-w", "-R", &format!("rmdir {LOST_FOUND}")]);
    debugfs_end(&run(rmdir.arg(dest))?)
}

const LOST_FOUND: &str = "lost+found";

#[cfg(test)]
mod tests {
    use super::debugfs_end;

    #[test]
    fn a_debugfs_request_fails_on_any_line_but_the_banner() {
        assert!(debugfs_end("").is_ok());
        assert!(debugfs_end("debugfs 1.47.0 (5-Feb-2023)\n").is_ok());
        let e = debugfs_end("debugfs 1.47.0 (5-Feb-2023)\nrmdir: directory not empty\n");
        assert_eq!(
            e.unwrap_err().to_string(),
            "debugfs failed: rmdir: directory not empty"
        );
    }
}
