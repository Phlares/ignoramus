//! Host side of level 2: VMM arguments, the one control connection, output read-back.
use crate::agent::steps::now_nanos;
use crate::image::{ext4, size_mib};
use crate::pack;
use crate::proto::{
    CONTROL_PORT, GUEST_CID, Job, Report, SERIAL_IN, SERIAL_OUT, SERIAL_ROOTFS, ToHost, read_frame,
    write_frame,
};
use crate::tree::{tree_hash, walk};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

pub const REPORT_SLACK: Duration = Duration::from_secs(30);

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
    pub output_hash: String,
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

/// Owns the VMM process. Dropping it kills and reaps the process.
struct Vmm(Child);

impl Vmm {
    /// Waits up to `grace` for the VMM to exit by itself, then kills it.
    fn stop(&mut self, grace: Duration) -> io::Result<()> {
        let end = Instant::now() + grace;
        while self.0.try_wait()?.is_none() {
            if Instant::now() >= end {
                self.0.kill()?;
                self.0.wait()?;
                break;
            }
            sleep(POLL);
        }
        Ok(())
    }
}

impl Drop for Vmm {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Waits for the one guest connection; fails early if the VMM dies first.
fn accept(
    listener: &UnixListener,
    vmm: &mut Vmm,
    started: Instant,
    vm: &Vm,
    work: &Path,
) -> Result<UnixStream, VmError> {
    loop {
        match listener.accept() {
            Ok((s, _)) => return Ok(s),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }
        if let Some(status) = vmm.0.try_wait()? {
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
        sleep(POLL);
    }
}

/// Reads from and writes to a stream until an absolute deadline. A guest that trickles
/// bytes or drains slowly cannot stretch the wait: each call gets only the time that is left.
struct Until<'a> {
    stream: &'a mut UnixStream,
    at: Instant,
}

/// Linux applies a send timeout to each socket buffer a write waits for, not to the whole
/// call. A write this small fills one buffer, so it waits at most once.
const WRITE_CHUNK: usize = 2048;

impl Until<'_> {
    fn left(&self) -> io::Result<Duration> {
        let left = self.at.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(left)
    }
}

impl Read for Until<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.left()?))?;
        self.stream.read(buf)
    }
}

impl Write for Until<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.left()?))?;
        self.stream.write(&buf[..buf.len().min(WRITE_CHUNK)])
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// Reads one frame before `at`. A timeout becomes `on_timeout(message)`; a closed or
/// broken channel becomes a guest error.
fn recv(
    s: &mut UnixStream,
    at: Instant,
    what: &str,
    on_timeout: fn(String) -> VmError,
    work: &Path,
) -> Result<ToHost, VmError> {
    read_frame(&mut Until { stream: s, at }).map_err(|e| match e.kind() {
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
    // A comma would split a Cloud Hypervisor option.
    let paths = [&vm.kernel, &vm.initramfs, &vm.rootfs]
        .into_iter()
        .chain(vm.serial_log.as_ref())
        .map(PathBuf::as_path)
        .chain([work]);
    for p in paths {
        if p.as_os_str().as_bytes().contains(&b',') {
            return Err(invalid(format!("path {} holds a comma", p.display())));
        }
    }
    for (what, dir) in [("work dir", work), ("output dir", &job.output_dir)] {
        if fs::read_dir(dir)?.next().is_some() {
            return Err(invalid(format!("{what} {} is not empty", dir.display())));
        }
    }
    // A tmpfs of size 0 has no limit, and /out and /tmp both live in guest memory.
    if vm.out_mib == 0 || vm.out_mib.saturating_mul(2) > u64::from(vm.mem_mib) {
        return Err(invalid(format!(
            "output disk of {} MiB: it needs at least 1 MiB and at most half of the {} MiB guest memory",
            vm.out_mib, vm.mem_mib
        )));
    }
    readable_by_others(&job.input_dir)
}

/// The payload runs as an unprivileged user, so every input below `dir` must be readable by
/// others: a file needs other-read, a directory other-read and other-execute. Links are not
/// followed.
fn readable_by_others(dir: &Path) -> io::Result<()> {
    walk(dir, &mut |_, path, ty| {
        let need = match ty {
            t if t.is_dir() => 0o005,
            t if t.is_symlink() => 0,
            _ => 0o004,
        };
        let mode = fs::symlink_metadata(path)?.mode();
        if mode & need != need {
            return Err(invalid(format!(
                "input {} has mode {:o}, which the unprivileged payload cannot read",
                path.display(),
                mode & 0o7777
            )));
        }
        Ok(())
    })
}

/// Runs one row in one VM. `work` must exist and be empty.
pub fn run_row(vm: &Vm, job: &HostJob, work: &Path) -> Result<VmOutcome, VmError> {
    let t0 = Instant::now();
    check_inputs(vm, job, work)?;
    let listen_at = listen_path(work);

    let in_img = work.join("in.img");
    ext4(Some(&job.input_dir), &in_img, size_mib(&job.input_dir)?)?;
    File::create(work.join("out.img"))?.set_len(vm.out_mib << 20)?;
    let prepare = t0.elapsed();

    let listener = UnixListener::bind(&listen_at)?;
    listener.set_nonblocking(true)?;
    let log = File::create(work.join("vmm.log"))?;
    let started = Instant::now();
    let mut vmm = Vmm(Command::new(&vm.vmm)
        .args(vmm_args(vm, work))
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()
        .map_err(|e| VmError::Boot(format!("cannot start {}: {e}", vm.vmm.display())))?);

    let mut chan = accept(&listener, &mut vmm, started, vm, work)?;
    // Later connects must fail: the self-test checks that no host path is left.
    drop(listener);
    fs::remove_file(&listen_at)?;
    chan.set_nonblocking(false)?;

    match recv(
        &mut chan,
        started + vm.boot_timeout,
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
    let wire = Job {
        seed_hex: hex::encode(job.seed),
        unix_nanos: now_nanos(),
        argv: job.argv.clone(),
        env: job.env.clone(),
        timeout_ms: job.wall_timeout.as_millis() as u64,
    };
    let report_by = sent + job.wall_timeout + vm.report_slack;
    let mut until = Until {
        stream: &mut chan,
        at: report_by,
    };
    write_frame(&mut until, &wire).map_err(|e| match e.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
            VmError::Timeout(with_log("the guest did not take the job".into(), work))
        }
        _ => guest_err(
            format!("control channel closed while sending the job: {e}"),
            work,
        ),
    })?;
    let report = match recv(&mut chan, report_by, "report", VmError::Timeout, work)? {
        ToHost::Report(r) => r,
        ToHost::Failed { reason } => {
            return Err(guest_err(format!("guest failed: {}", shown(&reason)), work));
        }
        ToHost::Hello { .. } => {
            return Err(guest_err("second hello instead of a report".into(), work));
        }
    };
    let guest = sent.elapsed();

    // The agent syncs the pack before it reports, so a VMM that lingers is killed.
    let stopping = Instant::now();
    vmm.stop(vm.exit_timeout)?;
    let stop = stopping.elapsed();

    let extracting = Instant::now();
    let mut output_hash = String::new();
    if report.end.is_some() {
        let mut out = BufReader::new(File::open(work.join("out.img"))?);
        pack::unpack(&mut out, &job.output_dir, vm.out_mib << 20).map_err(|e| match e.kind() {
            io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof => {
                guest_err(format!("bad output pack: {}", shown(&e.to_string())), work)
            }
            _ => VmError::Io(e),
        })?;
        output_hash = tree_hash(&job.output_dir)?;
        if output_hash != report.output_hash {
            return Err(VmError::Mismatch {
                guest: shown(&report.output_hash),
                host: output_hash,
            });
        }
    }
    let extract = extracting.elapsed();

    let payload = Duration::from_nanos(report.payload_wall_ns);
    Ok(VmOutcome {
        report,
        output_hash,
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
