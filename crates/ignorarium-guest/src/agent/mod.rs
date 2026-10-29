//! The guest agent. `main` runs as PID 1 of the guest; its steps are plain functions, so
//! most of them run without a virtual machine.
pub mod payload;
pub mod sandbox;
pub mod selftest;
pub mod steps;

use crate::pack;
use crate::proto::{
    CONTROL_PORT, HOST_CID, Job, Report, SERIAL_IN, SERIAL_OUT, SERIAL_ROOTFS, SelfTest, ToHost,
    read_frame, seed_from_hex, write_frame,
};
use crate::sys::{kill_all_others, vsock_connect};
use crate::tree::tree_hash;
use payload::Ran;
use rustix::io::Errno;
use rustix::mount::MountFlags;
use rustix::process::{WaitOptions, wait};
use rustix::system::{RebootCommand, reboot};
use rustix::thread::set_no_new_privs;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter};
use std::os::fd::OwnedFd;
use std::os::unix::fs::chroot;
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;
use steps::{disk_by_serial, disk_bytes, mix_seed, now_nanos, set_clock};

/// The payload's user and group: `nobody` and `nogroup` in Debian-based images.
pub const PAYLOAD_UID: u32 = 65534;
pub const PAYLOAD_GID: u32 = 65534;

const CONNECT_TRIES: u32 = 10;
const CONNECT_PAUSE: Duration = Duration::from_millis(50);

/// PID 1 of the guest: sets up, runs one row, reports, and powers off. It never exits:
/// PID 1 exiting is a kernel panic, and the VMM does not stop on a guest panic.
pub fn main() -> ! {
    if std::process::id() != 1 {
        eprintln!("ignorarium-guest runs only as PID 1 of a guest");
        std::process::exit(2);
    }
    // A panic has printed its message already.
    if let Ok(Err(e)) = std::panic::catch_unwind(run) {
        eprintln!("ignorarium-guest: {e}");
    }
    rustix::fs::sync();
    loop {
        // `reboot` returns only on an error; then wait for the host to stop the VMM.
        let _ = reboot(RebootCommand::PowerOff);
        sleep(Duration::from_secs(1));
    }
}

fn run() -> io::Result<()> {
    for (fs_type, dir) in [("proc", "/proc"), ("sysfs", "/sys"), ("devtmpfs", "/dev")] {
        let dir = Path::new(dir);
        fs::create_dir_all(dir)?;
        match sandbox::mount_fs(fs_type, dir, fs_type, MountFlags::empty(), "") {
            Err(e) if e.kind() != io::ErrorKind::ResourceBusy => return Err(e),
            _ => {}
        }
    }
    let sys_block = Path::new("/sys/block");
    let disk = |serial| disk_by_serial(sys_block, serial);
    let (rootfs, inputs, out) = (disk(SERIAL_ROOTFS)?, disk(SERIAL_IN)?, disk(SERIAL_OUT)?);
    let env = Path::new("/env");
    fs::create_dir_all(env)?;
    sandbox::mount_fs(&rootfs, env, "ext4", MountFlags::RDONLY, "")?;
    let out_disk = OpenOptions::new().write(true).open(&out)?;
    let out_bytes = disk_bytes(sys_block, &out)?;
    sandbox::build(env, Path::new("/sandbox"), &inputs, out_bytes)?;

    let mut chan = File::from(connect()?);
    let agent = env!("CARGO_PKG_VERSION").to_string();
    write_frame(&mut chan, &ToHost::Hello { agent })?;
    match row(&mut chan, out_disk) {
        Ok(report) => write_frame(&mut chan, &ToHost::Report(report)),
        Err(e) => {
            let reason = e.to_string();
            let _ = write_frame(&mut chan, &ToHost::Failed { reason });
            Err(e)
        }
    }
}

/// The host listens before it starts the VMM, so a failure means the device is not up yet.
fn connect() -> io::Result<OwnedFd> {
    for _ in 1..CONNECT_TRIES {
        if let Ok(fd) = vsock_connect(HOST_CID, CONTROL_PORT) {
            return Ok(fd);
        }
        sleep(CONNECT_PAUSE);
    }
    vsock_connect(HOST_CID, CONTROL_PORT)
}

/// Takes the job, seeds and sets the clock, then runs the payload behind the self-test and
/// packs `/out` onto the output disk.
fn row(chan: &mut File, mut out_disk: File) -> io::Result<Report> {
    let job: Job = read_frame(chan)?;
    let seed = seed_from_hex(&job.seed_hex)?;
    mix_seed(&seed, &OpenOptions::new().write(true).open("/dev/urandom")?)?;
    set_clock(job.unix_nanos)?;
    let clock = now_nanos();
    let net = Path::new("/sys/class/net");
    let selftest = selftest::run(net, || vsock_connect(HOST_CID, CONTROL_PORT).map(drop));
    gate(selftest, clock, || {
        chroot("/sandbox")?;
        std::env::set_current_dir("/")?;
        // Inherited by the payload, so no setuid file gives it root again.
        set_no_new_privs(true)?;
        let ran = payload::run_as(&job, Some((PAYLOAD_UID, PAYLOAD_GID)))?;
        kill_all_others()?;
        reap_all()?;
        let out = Path::new("/out");
        let hash = tree_hash(out)?;
        let mut w = BufWriter::new(&mut out_disk);
        pack::write(out, &mut w)?;
        w.into_inner()?.sync_all()?;
        Ok((ran, hash))
    })
}

/// Reaps children until none is left. After the kill every other process is a dying child
/// of PID 1, or becomes one when its parent dies.
fn reap_all() -> io::Result<()> {
    loop {
        match wait(WaitOptions::empty()) {
            Ok(_) | Err(Errno::INTR) => {}
            Err(Errno::CHILD) => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    }
}

/// Runs the payload only when the self-test passed. `run` returns the run and its output hash.
pub fn gate(
    selftest: SelfTest,
    clock_unix_nanos: u64,
    run: impl FnOnce() -> io::Result<(Ran, String)>,
) -> io::Result<Report> {
    let (end, payload_wall_ns, cpu_seconds, output_hash) = if selftest.ok() {
        let (ran, hash) = run()?;
        (
            Some(ran.end),
            ran.wall.as_nanos() as u64,
            ran.cpu_seconds,
            hash,
        )
    } else {
        (None, 0, 0.0, String::new())
    };
    Ok(Report {
        selftest,
        clock_unix_nanos,
        end,
        payload_wall_ns,
        cpu_seconds,
        output_hash,
    })
}
