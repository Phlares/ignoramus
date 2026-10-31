//! Boot steps: seed the kernel random pool, set the clock, find the disks.
use super::sandbox::tmpfs_bytes;
use crate::sys;
use rustix::time::{ClockId, Timespec, clock_settime};
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Writes `seed` into the pool, then forces a reseed so it takes effect at once.
pub fn mix_seed(seed: &[u8; 32], random: &File) -> io::Result<()> {
    let mut f = random;
    f.write_all(seed)?;
    reseeded(sys::reseed_crng(random.as_fd()))
}

/// The kernel refuses a reseed with `ENODATA` while its generator is not ready yet. The seed
/// is in the input pool by then and goes into the first seeding, so that counts as done.
fn reseeded(r: io::Result<()>) -> io::Result<()> {
    match r {
        Err(e) if e.raw_os_error() == Some(libc::ENODATA) => Ok(()),
        r => r,
    }
}

/// Turns io_uring off for every process, root included, so the self-test sees it refused as a
/// level 1 sandbox does. A kernel without the switch is left as it is; the self-test then
/// finds the call absent or open. The switch:
/// <https://docs.kernel.org/admin-guide/sysctl/kernel.html#io-uring-disabled>
pub fn disable_io_uring(switch: &Path) -> io::Result<()> {
    match fs::OpenOptions::new().write(true).open(switch) {
        Ok(mut f) => f.write_all(b"2"),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn set_clock(unix_nanos: u64) -> io::Result<()> {
    let ts = Timespec {
        tv_sec: (unix_nanos / 1_000_000_000) as _,
        tv_nsec: (unix_nanos % 1_000_000_000) as _,
    };
    Ok(clock_settime(ClockId::Realtime, ts)?)
}

pub fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// Size of the disk `/dev/<name>`. Sysfs gives it in 512-byte sectors whatever the block size.
pub fn disk_bytes(sys_block: &Path, dev: &Path) -> io::Result<u64> {
    let name = dev.file_name().unwrap_or_default();
    let text = fs::read_to_string(sys_block.join(name).join("size"))?;
    let sectors: u64 = text
        .trim()
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("disk size: {e}")))?;
    Ok(sectors * 512)
}

/// Finds the block device whose `serial` file holds `serial`.
pub fn disk_by_serial(sys_block: &Path, serial: &str) -> io::Result<PathBuf> {
    for entry in fs::read_dir(sys_block)? {
        let dev = entry?.file_name();
        let Ok(text) = fs::read_to_string(sys_block.join(&dev).join("serial")) else {
            continue;
        };
        if text.trim_end_matches(['\n', '\0']) == serial {
            return Ok(Path::new("/dev").join(dev));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("no disk with serial {serial}"),
    ))
}

/// Memory, in bytes, the guest must still have free at boot beyond what the row may hold: for
/// the payload's page tables and processes and the page cache.
pub const FLOOR: u64 = 128 << 20;

/// `MemAvailable` of the `/proc/meminfo` text at `meminfo`, in bytes.
pub fn mem_available(meminfo: &Path) -> io::Result<u64> {
    let text = fs::read_to_string(meminfo)?;
    let value = text.lines().find_map(|l| l.strip_prefix("MemAvailable:"));
    let kb = value.and_then(|v| v.trim().strip_suffix("kB")?.trim().parse::<u64>().ok());
    kb.map(|kb| kb << 10).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} names no MemAvailable", meminfo.display()),
        )
    })
}

/// Refuses a row the guest cannot hold: `available` bytes at boot against the payload's data
/// limit, the `/out` and `/tmp` tmpfs of an output disk of `out_bytes`, and `FLOOR`. The
/// host sizes the machine by a model of the kernel's own needs; this turns a model that
/// falls short into a named fault before the payload, never an out-of-memory kill.
pub fn room(available: u64, data_limit: Option<u64>, out_bytes: u64) -> io::Result<()> {
    let (data, tmpfs) = (data_limit.unwrap_or(0), tmpfs_bytes(out_bytes));
    let need = data.saturating_add(tmpfs).saturating_add(FLOOR);
    if available >= need {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "the guest has {} MiB available at boot, under the {} MiB the row needs: a data limit \
         of {} MiB, {} MiB for /out and /tmp, and a {} MiB floor",
        available >> 20,
        need.div_ceil(1 << 20),
        data >> 20,
        tmpfs >> 20,
        FLOOR >> 20
    )))
}

#[cfg(test)]
mod tests {
    use super::reseeded;
    use std::io;

    #[test]
    fn a_reseed_before_the_generator_is_ready_counts_as_done() {
        let err = |n| Err(io::Error::from_raw_os_error(n));
        assert!(reseeded(err(libc::ENODATA)).is_ok());
        let e = reseeded(err(libc::ENOTTY)).unwrap_err();
        assert_eq!(e.raw_os_error(), Some(libc::ENOTTY));
        assert!(reseeded(Ok(())).is_ok());
    }
}
