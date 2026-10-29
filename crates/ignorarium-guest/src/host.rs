//! Host side of level 2: VMM arguments, the one control connection, output read-back.
use crate::image::{ext4, size_mib};
use crate::pack;
use crate::proto::{
    CONTROL_PORT, GUEST_CID, Job, Report, SERIAL_IN, SERIAL_OUT, SERIAL_ROOTFS, ToHost, read_frame,
    write_frame,
};
use crate::tree::tree_hash;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    Mismatch { guest: String, host: String },
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
    vsock.push(kv("socket=", &work.join("vsock.sock")));
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

/// Reads one frame within `within`. A timeout becomes `on_timeout(what)`; a closed or
/// broken channel becomes a guest error.
fn recv(
    s: &mut UnixStream,
    within: Duration,
    what: &str,
    on_timeout: fn(String) -> VmError,
    work: &Path,
) -> Result<ToHost, VmError> {
    s.set_read_timeout(Some(within.max(Duration::from_millis(1))))?;
    read_frame(s).map_err(|e| match e.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
            on_timeout(format!("no {what} within {within:?}"))
        }
        io::ErrorKind::UnexpectedEof => {
            let msg = format!("control channel closed before the {what}");
            VmError::Guest(with_log(msg, work))
        }
        _ => {
            let msg = format!("control channel failed before the {what}: {e}");
            VmError::Guest(with_log(msg, work))
        }
    })
}

fn boot_timed_out(msg: String) -> VmError {
    VmError::Boot(format!("timed out: {msg}"))
}

/// Runs one row in one VM. `work` must exist and be empty.
pub fn run_row(vm: &Vm, job: &HostJob, work: &Path) -> Result<VmOutcome, VmError> {
    let t0 = Instant::now();
    let listen_path = work.join(format!("vsock.sock_{CONTROL_PORT}"));
    if listen_path.as_os_str().len() > SOCKET_PATH_MAX {
        return Err(invalid(format!(
            "work dir is too deep for a Unix socket: {} bytes, at most {SOCKET_PATH_MAX}",
            listen_path.as_os_str().len()
        ))
        .into());
    }
    if fs::read_dir(work)?.next().is_some() {
        return Err(invalid(format!("work dir {} is not empty", work.display())).into());
    }

    let in_img = work.join("in.img");
    ext4(Some(&job.input_dir), &in_img, size_mib(&job.input_dir)?)?;
    File::create(work.join("out.img"))?.set_len(vm.out_mib << 20)?;
    let prepare = t0.elapsed();

    let listener = UnixListener::bind(&listen_path)?;
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
    fs::remove_file(&listen_path)?;
    chan.set_nonblocking(false)?;

    let left = vm.boot_timeout.saturating_sub(started.elapsed());
    match recv(&mut chan, left, "hello", boot_timed_out, work)? {
        ToHost::Hello { .. } => {}
        ToHost::Failed { reason } => {
            return Err(VmError::Guest(with_log(
                format!("guest failed to start: {reason}"),
                work,
            )));
        }
        ToHost::Report(_) => {
            return Err(VmError::Guest(with_log("report before hello".into(), work)));
        }
    }
    let boot = started.elapsed();

    let sent = Instant::now();
    let wire = Job {
        seed_hex: hex::encode(job.seed),
        unix_nanos: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64),
        argv: job.argv.clone(),
        env: job.env.clone(),
        timeout_ms: job.wall_timeout.as_millis() as u64,
    };
    write_frame(&mut chan, &wire).map_err(|e| {
        VmError::Guest(with_log(
            format!("control channel closed while sending the job: {e}"),
            work,
        ))
    })?;
    let within = job.wall_timeout + vm.report_slack;
    let report = match recv(&mut chan, within, "report", VmError::Timeout, work)? {
        ToHost::Report(r) => r,
        ToHost::Failed { reason } => {
            return Err(VmError::Guest(with_log(
                format!("guest failed: {reason}"),
                work,
            )));
        }
        ToHost::Hello { .. } => {
            return Err(VmError::Guest(with_log(
                "second hello instead of a report".into(),
                work,
            )));
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
                VmError::Guest(format!("bad output pack: {e}"))
            }
            _ => VmError::Io(e),
        })?;
        output_hash = tree_hash(&job.output_dir)?;
        if output_hash != report.output_hash {
            return Err(VmError::Mismatch {
                guest: report.output_hash,
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
