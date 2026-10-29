//! Host-side parts of a guest image: pinned downloads, the initramfs and the ext4 disks.
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read};
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
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(hex::encode(h.finalize()));
        }
        h.update(&buf[..n]);
    }
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

/// A boot archive holding `/dev/console` and the agent as `/init`.
pub fn initramfs(agent: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    newc_entry(&mut out, 1, "dev", 0o040755, (0, 0), &[]);
    newc_entry(&mut out, 2, "dev/console", 0o020600, (5, 1), &[]);
    newc_entry(&mut out, 3, "init", 0o100755, (0, 0), agent);
    newc_entry(&mut out, 4, "TRAILER!!!", 0, (0, 0), &[]);
    out
}

const BLOCK: u64 = 4096;

fn tree_bytes(dir: &Path) -> io::Result<u64> {
    let mut sum = 0;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        sum += meta.len().next_multiple_of(BLOCK) + BLOCK;
        if meta.is_dir() {
            sum += tree_bytes(&entry.path())?;
        }
    }
    Ok(sum)
}

/// Image size for `dir`: each entry's size rounded up to a block, a block per entry
/// for inodes and directory slots, 25% slack, plus 16 MiB.
pub fn size_mib(dir: &Path) -> io::Result<u64> {
    Ok((tree_bytes(dir)? * 5 / 4).div_ceil(1 << 20) + 16)
}

fn run(cmd: &mut Command) -> io::Result<()> {
    let out = cmd.stdin(Stdio::null()).output()?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let tail: Vec<&str> = err.lines().rev().take(10).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    Err(io::Error::other(format!(
        "{:?} failed ({}): {}",
        cmd.get_program(),
        out.status,
        tail.join("\n")
    )))
}

/// Makes an ext4 image of `size_mib` MiB at `dest`, filled from `src`. It has no journal,
/// so reading it on the host never needs a replay.
pub fn ext4(src: Option<&Path>, dest: &Path, size_mib: u64) -> io::Result<()> {
    File::create(dest)?.set_len(size_mib << 20)?;
    let mut cmd = Command::new("mkfs.ext4");
    cmd.args(["-q", "-F", "-O", "^has_journal", "-E", "root_owner=0:0"]);
    if let Some(src) = src {
        cmd.arg("-d").arg(src);
    }
    run(cmd.arg(dest))
}

/// Copies the root of an ext4 image into the existing directory `dest`.
/// `debugfs -R` splits its command on whitespace, so `dest` may hold none and no quotes.
pub fn extract(img: &Path, dest: &Path) -> io::Result<()> {
    let text = dest
        .to_str()
        .filter(|t| !t.contains(|c: char| c.is_whitespace() || c == '"' || c == '\''))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "dest must be UTF-8 without whitespace or quotes",
            )
        })?;
    // As non-root, debugfs warns that it cannot change ownership and still exits 0.
    run(Command::new("debugfs")
        .arg("-R")
        .arg(format!("rdump / {text}"))
        .arg(img))
}
