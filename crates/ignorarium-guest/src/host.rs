//! Host side of level 2: VMM arguments, the one control connection, output read-back.
//!
//! The VMM leads its own process group. The host sees it exit with a wait that leaves it
//! unreaped, so a caller must keep SIGCHLD at its default action: an ignored SIGCHLD lets the
//! kernel reap the VMM first.
use crate::agent::payload::exited;
use crate::agent::steps::now_nanos;
use crate::image::{ext4, size_mib};
use crate::pack::{self, Link, Unpacked};
use crate::proto::{
    CONTROL_PORT, GUEST_CID, Job, MAX_FRAME, Report, SERIAL_IN, SERIAL_OUT, SERIAL_PAYLOAD,
    SERIAL_ROOTFS, ToHost, read_frame, write_frame,
};
use crate::sys::{Ended, wait4};
use crate::tree::walk;
use rustix::io::Errno;
use rustix::process::{Pid, Signal, kill_process, kill_process_group};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

pub const REPORT_SLACK: Duration = Duration::from_secs(30);

/// What the caller hears while a row runs.
pub trait Watch {
    /// Called between the steps that build the disks and every `POLL` while the host waits
    /// on the VMM or the guest. `false` stops the row.
    fn tick(&self) -> bool;
    /// The VMM now runs as the leader of its own process group, `pid`. Called once per row.
    fn started(&self, _pid: u32) {}
    /// The VMM's group is killed and its leader reaped, after `cpu_seconds` of user and system
    /// time: its threads and the children it reaped. Called once for each `started`.
    fn reaped(&self, _cpu_seconds: f64) {}
}

impl<F: Fn() -> bool> Watch for F {
    fn tick(&self) -> bool {
        self()
    }
}

pub struct Vm {
    pub vmm: PathBuf,
    pub kernel: PathBuf,
    pub initramfs: PathBuf,
    pub rootfs: PathBuf,
    pub cpus: u32,
    pub mem_mib: u32,
    pub out_mib: u64,
    pub boot_timeout: Duration,
    pub exit_timeout: Duration,
    pub report_slack: Duration,
    pub serial_log: Option<PathBuf>,
}

pub struct HostJob {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub input_dir: PathBuf,
    /// Reaches the guest read-only at `proto::GUEST_PAYLOAD`.
    pub payload_dir: PathBuf,
    pub output_dir: PathBuf,
    pub seed: [u8; 32],
    pub wall_timeout: Duration,
}

#[derive(Debug, Clone, Default)]
pub struct Timings {
    pub prepare: Duration,
    pub boot: Duration,
    pub guest: Duration,
    pub payload: Duration,
    pub stop: Duration,
    pub extract: Duration,
    pub total: Duration,
}

#[derive(Debug)]
pub struct VmOutcome {
    pub report: Report,
    /// The hash of the output pack as the host read it.
    pub output_hash: String,
    /// The output links in pack order. The host records them and makes none.
    pub links: Vec<Link>,
    pub timings: Timings,
}

#[derive(Debug)]
pub enum VmError {
    Boot(String),
    Guest(String),
    Timeout(String),
    /// `guest` is the hash the guest reported, capped, quoted and escaped.
    Mismatch {
        guest: String,
        host: String,
    },
    Io(io::Error),
    /// The watch said stop; the VMM's process group is killed.
    Stopped,
}

impl From<io::Error> for VmError {
    fn from(e: io::Error) -> Self {
        VmError::Io(e)
    }
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmError::Boot(m) => write!(f, "boot failed: {m}"),
            VmError::Guest(m) => write!(f, "guest error: {m}"),
            VmError::Timeout(m) => write!(f, "timed out: {m}"),
            VmError::Mismatch { guest, host } => {
                write!(
                    f,
                    "output hash mismatch: guest said {guest}, host computed {host}"
                )
            }
            VmError::Io(e) => write!(f, "host I/O error: {e}"),
            VmError::Stopped => write!(f, "stopped by the box"),
        }
    }
}

impl std::error::Error for VmError {}

/// A Unix socket path holds 107 bytes plus the NUL.
const SOCKET_PATH_MAX: usize = 107;
const LOG_TAIL_CHARS: usize = 2000;
const POLL: Duration = Duration::from_millis(5);
/// Guest-written text enters host errors at most this long.
const SHOWN_CHARS: usize = 200;
/// The VMM's vsock socket in the work dir. A guest connect to port P reaches the host at
/// `<socket>_P`.
const VSOCK_SOCKET: &str = "vsock.sock";

fn listen_path(work: &Path) -> PathBuf {
    work.join(format!("{VSOCK_SOCKET}_{CONTROL_PORT}"))
}

/// Guest-written text for an error message: at most `SHOWN_CHARS` characters, escaped.
fn shown(s: &str) -> String {
    let mut out: String = s.chars().take(SHOWN_CHARS).collect();
    if s.chars().nth(SHOWN_CHARS).is_some() {
        out.push_str("...");
    }
    format!("{out:?}")
}

/// A guest-side failure, with the VMM log tail.
fn guest_err(msg: String, work: &Path) -> VmError {
    VmError::Guest(with_log(msg, work))
}

fn kv(key: &str, path: &Path) -> OsString {
    let mut s = OsString::from(key);
    s.push(path);
    s
}

fn disk(path: &Path, readonly: bool, serial: &str) -> OsString {
    let mut s = kv("path=", path);
    s.push(if readonly { ",readonly=on" } else { "" });
    s.push(format!(",image_type=raw,serial={serial}"));
    s
}

/// Cloud Hypervisor arguments for one row. No network device, no API socket.
pub fn vmm_args(vm: &Vm, work: &Path) -> Vec<OsString> {
    let (cmdline, serial) = match &vm.serial_log {
        Some(log) => ("console=ttyS0 quiet", kv("file=", log)),
        None => ("quiet", OsString::from("off")),
    };
    let mut vsock = OsString::from(format!("cid={GUEST_CID},"));
    vsock.push(kv("socket=", &work.join(VSOCK_SOCKET)));
    let mut args: Vec<OsString> = vec![
        "--kernel".into(),
        vm.kernel.clone().into(),
        "--initramfs".into(),
        vm.initramfs.clone().into(),
        "--cmdline".into(),
        cmdline.into(),
        "--cpus".into(),
        format!("boot={}", vm.cpus).into(),
        "--memory".into(),
        format!("size={}M", vm.mem_mib).into(),
        "--disk".into(),
    ];
    args.push(disk(&vm.rootfs, true, SERIAL_ROOTFS));
    args.push(disk(&work.join("in.img"), true, SERIAL_IN));
    args.push(disk(&work.join("out.img"), false, SERIAL_OUT));
    args.push(disk(&work.join("payload.img"), true, SERIAL_PAYLOAD));
    args.extend(["--vsock".into(), vsock, "--console".into(), "off".into()]);
    args.extend(["--serial".into(), serial]);
    args
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

fn log_tail(work: &Path) -> String {
    let log = fs::read(work.join("vmm.log")).unwrap_or_default();
    let log = String::from_utf8_lossy(&log);
    let skip = log.chars().count().saturating_sub(LOG_TAIL_CHARS);
    log.chars().skip(skip).collect()
}

fn with_log(msg: String, work: &Path) -> String {
    format!("{msg}\nvmm.log tail:\n{}", log_tail(work))
}

/// Owns the VMM, which leads its own process group. Every way out, a drop included, kills
/// the group before it reaps the leader: until the reap, the leader's pid names the group,
/// so no unrelated group can take that id. The watch hears the group start, and the leader's
/// CPU once it is reaped.
struct Vmm<'a> {
    pid: Pid,
    watch: &'a dyn Watch,
    killed: bool,
    /// How the leader ended, once reaped.
    ended: Option<Ended>,
}

impl<'a> Vmm<'a> {
    fn spawn(cmd: &mut Command, watch: &'a dyn Watch) -> io::Result<Vmm<'a>> {
        let child = cmd.process_group(0).spawn()?;
        watch.started(child.id());
        Ok(Vmm {
            pid: Pid::from_child(&child),
            watch,
            killed: false,
            ended: None,
        })
    }

    /// Waits up to `grace` for the VMM to exit by itself, then ends it; at once on a stop.
    fn stop(&mut self, grace: Duration, watch: &dyn Watch) -> io::Result<()> {
        let end = Instant::now() + grace;
        while Instant::now() < end && !exited(self.pid)? && watch.tick() {
            sleep(POLL);
        }
        self.end().map(drop)
    }

    /// Kills the group, then reaps the leader. A group kill that fails for any reason but a
    /// group already gone is an error, after the reap. A later call returns the same ending.
    fn end(&mut self) -> io::Result<Ended> {
        if let Some(ended) = self.ended {
            return Ok(ended);
        }
        let mut fault = None;
        if !self.killed {
            self.killed = true;
            fault = kill_process_group(self.pid, Signal::KILL)
                .err()
                .filter(|&e| e != Errno::SRCH);
            // A leader that moved to another group is not in this one.
            let _ = kill_process(self.pid, Signal::KILL);
        }
        let reaped = wait4(self.pid.as_raw_nonzero().get() as u32)?;
        self.ended = Some(reaped.ended);
        self.watch.reaped(reaped.cpu_seconds);
        fault.map_or(Ok(reaped.ended), |e| Err(e.into()))
    }
}

impl Drop for Vmm<'_> {
    fn drop(&mut self) {
        let _ = self.end();
    }
}

fn go_on(watch: &dyn Watch) -> Result<(), VmError> {
    if watch.tick() {
        Ok(())
    } else {
        Err(VmError::Stopped)
    }
}

/// Waits for the one guest connection; fails early if the VMM dies first.
fn accept(
    listener: &UnixListener,
    vmm: &mut Vmm<'_>,
    started: Instant,
    vm: &Vm,
    work: &Path,
    watch: &dyn Watch,
) -> Result<UnixStream, VmError> {
    loop {
        match listener.accept() {
            Ok((s, _)) => return Ok(s),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }
        if exited(vmm.pid)? {
            let status = vmm.end()?;
            let msg = format!("the VMM exited ({status}) before the guest connected");
            return Err(VmError::Boot(with_log(msg, work)));
        }
        if started.elapsed() >= vm.boot_timeout {
            let msg = format!(
                "timed out after {:?} waiting for the guest",
                vm.boot_timeout
            );
            return Err(VmError::Boot(with_log(msg, work)));
        }
        go_on(watch)?;
        sleep(POLL);
    }
}

/// Reads from and writes to a stream until an absolute deadline. A guest that trickles
/// bytes or drains slowly cannot stretch the wait: each call gets only the time that is left.
/// A socket wait lasts at most `POLL`, and the watch ticks once `POLL` has passed since its
/// last tick, bytes or none, so a guest that trickles cannot hold off a stop. A wait that
/// moves no bytes never ends a call, so a frame that arrives in pieces stays whole.
struct Until<'a> {
    stream: &'a mut UnixStream,
    at: Instant,
    watch: &'a dyn Watch,
    last_tick: Instant,
    /// Set once the watch said stop.
    stopped: bool,
}

/// Linux applies a send timeout to each socket buffer a write waits for, not to the whole
/// call. A write this small fills one buffer, so it waits at most once.
const WRITE_CHUNK: usize = 2048;

impl<'a> Until<'a> {
    fn new(stream: &'a mut UnixStream, at: Instant, watch: &'a dyn Watch) -> Until<'a> {
        Until {
            stream,
            at,
            watch,
            last_tick: Instant::now(),
            stopped: false,
        }
    }

    /// Runs `op` with a socket timeout until it moves bytes, the deadline passes
    /// (`TimedOut`) or the watch says stop.
    fn wait<T>(
        &mut self,
        mut op: impl FnMut(&mut UnixStream, Duration) -> io::Result<T>,
    ) -> io::Result<T> {
        loop {
            let left = self.at.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            let done = op(self.stream, left.min(POLL));
            if self.last_tick.elapsed() >= POLL {
                self.last_tick = Instant::now();
                if !self.watch.tick() {
                    self.stopped = true;
                    return Err(io::Error::other("stopped"));
                }
            }
            match done {
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                done => return done,
            }
        }
    }

    /// `r` with its error as a `VmError`: `Stopped` after a stop, else `other`'s.
    fn mapped<T>(
        &self,
        r: io::Result<T>,
        other: impl FnOnce(io::Error) -> VmError,
    ) -> Result<T, VmError> {
        r.map_err(|e| {
            if self.stopped {
                VmError::Stopped
            } else {
                other(e)
            }
        })
    }
}

impl Read for Until<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.wait(|s, t| {
            s.set_read_timeout(Some(t))?;
            s.read(buf)
        })
    }
}

impl Write for Until<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.wait(|s, t| {
            s.set_write_timeout(Some(t))?;
            s.write(&buf[..buf.len().min(WRITE_CHUNK)])
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// Reads one frame before `at`. A timeout becomes `on_timeout(message)`; a closed or
/// broken channel becomes a guest error.
fn recv(
    stream: &mut UnixStream,
    at: Instant,
    watch: &dyn Watch,
    what: &str,
    on_timeout: fn(String) -> VmError,
    work: &Path,
) -> Result<ToHost, VmError> {
    let mut until = Until::new(stream, at, watch);
    let frame = read_frame(&mut until);
    until.mapped(frame, |e| match e.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => on_timeout(with_log(
            format!("no complete {what} before the deadline"),
            work,
        )),
        io::ErrorKind::UnexpectedEof => {
            guest_err(format!("control channel closed before the {what}"), work)
        }
        // Parser errors can quote guest bytes.
        _ => guest_err(
            format!(
                "control channel failed before the {what}: {}",
                shown(&e.to_string())
            ),
            work,
        ),
    })
}

fn boot_timed_out(msg: String) -> VmError {
    VmError::Boot(format!("timed out: {msg}"))
}

fn check_inputs(vm: &Vm, job: &HostJob, work: &Path) -> io::Result<()> {
    let listen_len = listen_path(work).as_os_str().len();
    if listen_len > SOCKET_PATH_MAX {
        return Err(invalid(format!(
            "work dir is too deep for a Unix socket: {listen_len} bytes, at most {SOCKET_PATH_MAX}"
        )));
    }
    // Cloud Hypervisor v54 splits an option at a comma and fails on a bracket or a quote.
    let paths = [&vm.kernel, &vm.initramfs, &vm.rootfs]
        .into_iter()
        .chain(vm.serial_log.as_ref())
        .map(PathBuf::as_path)
        .chain([work]);
    for p in paths {
        let bytes = p.as_os_str().as_bytes();
        if let Some(&c) = bytes.iter().find(|b| b",[]\"".contains(b)) {
            return Err(invalid(format!(
                "path {} holds {:?}, which Cloud Hypervisor cannot parse in an option",
                p.display(),
                char::from(c)
            )));
        }
    }
    for (what, dir) in [("work dir", work), ("output dir", &job.output_dir)] {
        if fs::read_dir(dir)?.next().is_some() {
            return Err(invalid(format!("{what} {} is not empty", dir.display())));
        }
    }
    // A tmpfs of size 0 has no limit. /out and /tmp live in guest memory, each up to the
    // disk size, and together they may take at most half of it.
    if vm.out_mib == 0 || vm.out_mib.saturating_mul(4) > u64::from(vm.mem_mib) {
        return Err(invalid(format!(
            "output disk of {} MiB: it needs at least 1 MiB and at most a quarter of the {} MiB guest memory",
            vm.out_mib, vm.mem_mib
        )));
    }
    // The clock field is widest at its maximum, so the job sent later is no longer.
    let job_len = serde_json::to_vec(&wire_job(job, u64::MAX))?.len();
    if job_len > MAX_FRAME {
        return Err(invalid(format!(
            "the job is {job_len} bytes of JSON, over the {MAX_FRAME}-byte frame limit"
        )));
    }
    readable_by_others("input", &job.input_dir)?;
    readable_by_others("payload", &job.payload_dir)
}

/// The job as the guest receives it.
fn wire_job(job: &HostJob, unix_nanos: u64) -> Job {
    Job {
        seed_hex: hex::encode(job.seed),
        unix_nanos,
        argv: job.argv.clone(),
        env: job.env.clone(),
        timeout_ms: job.wall_timeout.as_millis() as u64,
    }
}

/// The payload runs as an unprivileged user, so `dir` and every entry below it must be
/// readable by others: a file needs other-read, a directory other-read and other-execute.
/// Links below `dir` are not followed. `what` names the tree in the refusal.
fn readable_by_others(what: &str, dir: &Path) -> io::Result<()> {
    let check = |path: &Path, mode: u32, need: u32| {
        if mode & need == need {
            return Ok(());
        }
        Err(invalid(format!(
            "{what} {} has mode {:o}, which the unprivileged payload cannot read",
            path.display(),
            mode & 0o7777
        )))
    };
    check(dir, fs::metadata(dir)?.mode(), 0o005)?;
    walk(dir, &mut |_, path, ty| {
        let need = match ty {
            t if t.is_dir() => 0o005,
            t if t.is_symlink() => 0,
            _ => 0o004,
        };
        check(path, fs::symlink_metadata(path)?.mode(), need)
    })
}

/// Runs one row in one VM. `work` must exist and be empty.
pub fn run_row(
    vm: &Vm,
    job: &HostJob,
    work: &Path,
    watch: &dyn Watch,
) -> Result<VmOutcome, VmError> {
    let t0 = Instant::now();
    check_inputs(vm, job, work)?;
    let listen_at = listen_path(work);

    for (dir, image) in [
        (&job.input_dir, "in.img"),
        (&job.payload_dir, "payload.img"),
    ] {
        ext4(Some(dir), &work.join(image), size_mib(dir)?)?;
        go_on(watch)?;
    }
    File::create(work.join("out.img"))?.set_len(vm.out_mib << 20)?;
    let prepare = t0.elapsed();

    let listener = UnixListener::bind(&listen_at)?;
    listener.set_nonblocking(true)?;
    let log = File::create(work.join("vmm.log"))?;
    let started = Instant::now();
    let mut cmd = Command::new(&vm.vmm);
    cmd.args(vmm_args(vm, work))
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    let mut vmm = Vmm::spawn(&mut cmd, watch)
        .map_err(|e| VmError::Boot(format!("cannot start {}: {e}", vm.vmm.display())))?;

    let mut chan = accept(&listener, &mut vmm, started, vm, work, watch)?;
    // Later connects must fail: the self-test checks that no host path is left.
    drop(listener);
    fs::remove_file(&listen_at)?;
    chan.set_nonblocking(false)?;

    match recv(
        &mut chan,
        started + vm.boot_timeout,
        watch,
        "hello",
        boot_timed_out,
        work,
    )? {
        ToHost::Hello { .. } => {}
        ToHost::Failed { reason } => {
            return Err(guest_err(
                format!("guest failed to start: {}", shown(&reason)),
                work,
            ));
        }
        ToHost::Report(_) => {
            return Err(guest_err("report before hello".into(), work));
        }
    }
    let boot = started.elapsed();

    let sent = Instant::now();
    let wire = wire_job(job, now_nanos());
    let report_by = sent + job.wall_timeout + vm.report_slack;
    let mut until = Until::new(&mut chan, report_by, watch);
    let sent_job = write_frame(&mut until, &wire);
    until.mapped(sent_job, |e| match e.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
            VmError::Timeout(with_log("the guest did not take the job".into(), work))
        }
        _ => guest_err(
            format!("control channel closed while sending the job: {e}"),
            work,
        ),
    })?;
    let mut report = match recv(
        &mut chan,
        report_by,
        watch,
        "report",
        VmError::Timeout,
        work,
    )? {
        ToHost::Report(r) => r,
        ToHost::Failed { reason } => {
            return Err(guest_err(format!("guest failed: {}", shown(&reason)), work));
        }
        ToHost::Hello { .. } => {
            return Err(guest_err("second hello instead of a report".into(), work));
        }
    };
    let guest = sent.elapsed();
    // The guest wrote it, and it reaches the caller's records.
    report.pack_refused = report.pack_refused.as_deref().map(shown);

    // The agent syncs the pack before it reports, so a VMM that lingers is killed.
    let stopping = Instant::now();
    vmm.stop(vm.exit_timeout, watch)?;
    let stop = stopping.elapsed();

    let extracting = Instant::now();
    let mut unpacked = Unpacked::default();
    if report.end.is_some() && report.pack_refused.is_none() {
        let mut out = BufReader::new(File::open(work.join("out.img"))?);
        let bad_pack = |e: io::Error| match e.kind() {
            io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof => {
                guest_err(format!("bad output pack: {}", shown(&e.to_string())), work)
            }
            _ => VmError::Io(e),
        };
        unpacked = pack::unpack(&mut out, &job.output_dir, vm.out_mib << 20).map_err(bad_pack)?;
        if unpacked.hash != report.output_hash {
            return Err(VmError::Mismatch {
                guest: shown(&report.output_hash),
                host: unpacked.hash,
            });
        }
    }
    let extract = extracting.elapsed();

    let payload = Duration::from_nanos(report.payload_wall_ns);
    Ok(VmOutcome {
        report,
        output_hash: unpacked.hash,
        links: unpacked.links,
        timings: Timings {
            prepare,
            boot,
            guest,
            payload,
            stop,
            extract,
            total: t0.elapsed(),
        },
    })
}
