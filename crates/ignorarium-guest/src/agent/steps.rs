//! Boot steps: seed the kernel random pool, set the clock, find the disks.
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
    sys::reseed_crng(random.as_fd())
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
