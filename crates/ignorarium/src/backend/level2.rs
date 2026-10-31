//! Level 2: each row runs in a fresh Cloud Hypervisor virtual machine, through
//! `ignorarium_guest::host::run_row`. The environment, the inputs and the payload's folder
//! reach the guest as read-only disks; the outputs come back as a pack. Cloud Hypervisor:
//! <https://github.com/cloud-hypervisor/cloud-hypervisor>.
use super::rootfs;
use super::selftest::{Call, SelfTest};
use super::{Backend, Exit, Probe, RowJob, RowOutcome, TIMEOUT_CODE, Watch};
use crate::files::temp_beside;
use ignorarium_guest::host::{self, HostJob, Timings, Vm, VmError, VmOutcome};
use ignorarium_guest::image::{self, KERNEL, Pin, VMM, check_pin, has_interp};
use ignorarium_guest::proto::{self, End, GUEST_IN, GUEST_OUT, GUEST_PAYLOAD};
use ignorarium_guest::tree;
use rustix::io::Errno;
use rustix::process::{Pid, geteuid, test_kill_process};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const VMM_VAR: &str = "IGNORARIUM_VM_VMM";
const KERNEL_VAR: &str = "IGNORARIUM_VM_KERNEL";
const AGENT_VAR: &str = "IGNORARIUM_VM_AGENT";
const NO_REPORT: &str = "the guest sent no report, so the payload's CPU time is unknown";
const BAD_CPU: &str =
    "the guest reported a CPU time it cannot have used, so the payload's CPU time is unknown";
/// The prefix of a row's scratch folder in the temp folder; the box's pid and a count follow.
const SCRATCH: &str = "ignorarium-vm-";

/// Rows this process has started, so each gets its own scratch folder.
static ROWS: AtomicU64 = AtomicU64::new(0);

/// The level 2 backend. The hypervisor, the guest kernel and the agent are files on the
/// host; the probe checks the first two against their pins and the agent for a static build.
pub struct Level2 {
    /// The KVM device.
    pub kvm: PathBuf,
    /// Cloud Hypervisor.
    pub vmm: PathBuf,
    /// The guest kernel.
    pub kernel: PathBuf,
    /// A static build of the `ignorarium-guest` binary, the guest's `/init`.
    pub agent: PathBuf,
    /// Each row's machine.
    pub cpus: u32,
    pub mem_mib: u32,
    pub out_mib: u64,
    pub boot_timeout: Duration,
    pub exit_timeout: Duration,
    pub report_slack: Duration,
    /// The first line of `<vmm> --version`, read on first use.
    version: OnceLock<Option<String>>,
    /// The initramfs that holds the agent, and the agent's hex SHA-256, from the first read
    /// that works.
    initramfs: OnceLock<(Vec<u8>, String)>,
}

impl Default for Level2 {
    fn default() -> Self {
        Level2::new()
    }
}

impl Level2 {
    /// The files that `IGNORARIUM_VM_VMM`, `IGNORARIUM_VM_KERNEL` and `IGNORARIUM_VM_AGENT`
    /// name. An unset one leaves an empty path, which the probe names.
    pub fn new() -> Level2 {
        let var = |name| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .unwrap_or_default()
        };
        Level2::with(var(VMM_VAR), var(KERNEL_VAR), var(AGENT_VAR))
    }

    pub fn with(vmm: PathBuf, kernel: PathBuf, agent: PathBuf) -> Level2 {
        Level2 {
            kvm: "/dev/kvm".into(),
            vmm,
            kernel,
            agent,
            cpus: 1,
            mem_mib: 1024,
            out_mib: 256,
            boot_timeout: Duration::from_secs(30),
            exit_timeout: Duration::from_secs(10),
            report_slack: host::REPORT_SLACK,
            version: OnceLock::new(),
            initramfs: OnceLock::new(),
        }
    }

    /// Every reason this host cannot run level 2, in a fixed order.
    fn faults(&self) -> Vec<String> {
        let mut faults = Vec::new();
        let kvm = self.kvm.display();
        match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.kvm)
        {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                faults.push(format!("{kvm} is missing: level 2 needs KVM"));
            }
            Err(e) => faults.push(format!("cannot open {kvm}: {e}")),
        }
        let file =
            |var: &str, path: &Path, wanted: &str, check: &dyn Fn(&Path) -> Option<String>| {
                if path.as_os_str().is_empty() {
                    Some(format!("{var} is not set; point it at {wanted}"))
                } else {
                    check(path)
                }
            };
        faults.extend(file(VMM_VAR, &self.vmm, VMM.name, &|p| {
            runnable(p).or_else(|| pinned(p, &VMM))
        }));
        faults.extend(file(KERNEL_VAR, &self.kernel, KERNEL.name, &|p| {
            pinned(p, &KERNEL)
        }));
        let agent = "a static build of ignorarium-guest";
        faults.extend(file(AGENT_VAR, &self.agent, agent, &static_agent));
        // The disks are built with e2fsprogs.
        for tool in ["mkfs.ext4", "debugfs"] {
            let ran = Command::new(tool)
                .arg("-V")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if let Err(e) = ran {
                faults.push(format!("{tool} cannot run: {e}"));
            }
        }
        faults
    }

    /// The initramfs that holds the agent, and the agent's hex SHA-256. A failed read is
    /// tried again on the next call.
    fn initramfs(&self) -> Result<&(Vec<u8>, String), String> {
        if let Some(built) = self.initramfs.get() {
            return Ok(built);
        }
        let agent = fs::read(&self.agent)
            .map_err(|e| format!("cannot read the agent {}: {e}", self.agent.display()))?;
        let sha256 = hex::encode(Sha256::digest(&agent));
        Ok(self
            .initramfs
            .get_or_init(|| (image::initramfs(&agent), sha256)))
    }

    /// Runs one row and returns its outcome, with the host's timings when the machine ran to
    /// a report. The rootfs disk and the boot count toward the row's wall time, not the
    /// payload's.
    pub fn run_timed(&self, job: &RowJob, watch: &dyn Watch) -> (RowOutcome, Option<Timings>) {
        let start = Instant::now();
        let wall = || start.elapsed().as_secs_f64();
        let ended = |res: Result<VmOutcome, VmError>| {
            let timings = res.as_ref().ok().map(|o| o.timings.clone());
            (outcome_of(res, wall(), self.cpus), timings)
        };
        let infra = |why: String| (RowOutcome::failed(Exit::Infra(why), wall()), None);
        let Some(root) = &job.rootfs else {
            return infra("level 2 runs rows in an environment image; this study has none".into());
        };
        let host_job = match guest_job(job) {
            Ok(j) => j,
            Err(why) => return infra(why),
        };
        // prepare builds the disk; a row builds it again only when it went since.
        if !watch.tick() {
            return ended(Err(VmError::Stopped));
        }
        let cannot = |why: &dyn std::fmt::Display| {
            let name = rootfs::name_of(root);
            infra(format!(
                "cannot build the disk of environment {name}: {why}"
            ))
        };
        let rootfs = match rootfs_disk(root) {
            Ok(Ok(d)) => d,
            Ok(Err(why)) => return cannot(&why),
            Err(e) => return cannot(&e),
        };
        if !watch.tick() {
            return ended(Err(VmError::Stopped));
        }
        let initramfs = match self.initramfs() {
            Ok((b, _)) => b,
            Err(why) => return infra(why),
        };
        let scratch = match Scratch::new(initramfs) {
            Ok(s) => s,
            Err(e) => return infra(format!("cannot make the row's scratch folder: {e}")),
        };
        let vm = Vm {
            vmm: self.vmm.clone(),
            kernel: self.kernel.clone(),
            initramfs: scratch.0.join("initramfs.cpio"),
            rootfs,
            cpus: self.cpus,
            mem_mib: self.mem_mib,
            out_mib: self.out_mib,
            boot_timeout: self.boot_timeout,
            exit_timeout: self.exit_timeout,
            report_slack: self.report_slack,
            serial_log: None,
        };
        let counted = Counted {
            watch,
            cpu: Cell::new(0.0),
        };
        let res = host::run_row(&vm, &host_job, &scratch.0.join("work"), &counted);
        let (mut o, timings) = ended(res);
        o.box_cpu_seconds = counted.cpu.get();
        (o, timings)
    }
}

/// The runner's watch, and the CPU of the VMM once it is reaped.
struct Counted<'a> {
    watch: &'a dyn Watch,
    cpu: Cell<f64>,
}

impl Watch for Counted<'_> {
    fn tick(&self) -> bool {
        self.watch.tick()
    }
    fn started(&self, pid: u32) {
        self.watch.started(pid);
    }
    fn reaped(&self, cpu_seconds: f64) {
        self.cpu.set(self.cpu.get() + cpu_seconds);
    }
}

/// Why `path` is not the file `pin` names, if it is not.
fn pinned(path: &Path, pin: &Pin) -> Option<String> {
    match check_pin(path, pin) {
        Ok(()) => None,
        // A mismatch: the message names the pin, the path and both hashes.
        Err(e) if e.kind() == io::ErrorKind::InvalidData => Some(e.to_string()),
        Err(e) => Some(format!("{}: {e}", path.display())),
    }
}

/// Why the box cannot start `path`, if it cannot: no execute bit at all.
fn runnable(path: &Path) -> Option<String> {
    match fs::metadata(path) {
        Ok(m) if m.mode() & 0o111 != 0 => None,
        Ok(m) => Some(format!(
            "{} has mode {:o}, so it cannot run",
            path.display(),
            m.mode() & 0o7777
        )),
        Err(e) => Some(format!("{}: {e}", path.display())),
    }
}

/// Why `path` cannot be the guest's `/init`, if it cannot: the initramfs holds no loader.
fn static_agent(path: &Path) -> Option<String> {
    match fs::read(path).and_then(|elf| has_interp(&elf)) {
        Ok(false) => None,
        Ok(true) => Some(format!(
            "{} is linked dynamically, so it cannot run as the guest's /init",
            path.display()
        )),
        Err(e) => Some(format!("{}: {e}", path.display())),
    }
}

/// The first line of `<vmm> --version` when it names Cloud Hypervisor.
fn version(vmm: &Path) -> Option<String> {
    let out = Command::new(vmm)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next()?.trim();
    line.starts_with("cloud-hypervisor v")
        .then(|| line.to_string())
}

impl Backend for Level2 {
    fn level(&self) -> u8 {
        2
    }

    fn probe(&self) -> Probe {
        let faults = self.faults();
        if faults.is_empty() {
            Probe::Available
        } else {
            Probe::Unavailable(faults.join("; "))
        }
    }

    /// The vCPUs of each row's machine.
    fn cpus(&self) -> u32 {
        self.cpus
    }

    fn launcher(&self) -> String {
        let v = self.version.get_or_init(|| version(&self.vmm));
        v.clone()
            .unwrap_or_else(|| "cloud-hypervisor unknown".into())
    }

    /// The agent the rows boot: read once, with the initramfs.
    fn agent_sha256(&self) -> Option<String> {
        self.initramfs().ok().map(|(_, sha256)| sha256.clone())
    }

    fn run_row(&self, job: &RowJob, watch: &dyn Watch) -> io::Result<RowOutcome> {
        Ok(self.run_timed(job, watch).0)
    }
}

/// The job as the guest runs it: the folder of `argv[0]` is the payload disk, which the
/// guest mounts at `/payload`, and the payload sees the guest's paths. The guest runs the
/// payload as an unprivileged user, so `argv[0]` needs other-execute.
pub fn guest_job(job: &RowJob) -> Result<HostJob, String> {
    let (prog, rest) = job.argv.split_first().ok_or("empty argv")?;
    let prog = Path::new(prog);
    let (Some(dir), Some(name)) = (prog.parent(), prog.file_name()) else {
        return Err(format!("payload {} names no file", prog.display()));
    };
    let mode = fs::metadata(prog)
        .map_err(|e| format!("payload {}: {e}", prog.display()))?
        .mode();
    if mode & 0o001 == 0 {
        return Err(format!(
            "payload {} has mode {:o}, which the guest's unprivileged user cannot run",
            prog.display(),
            mode & 0o7777
        ));
    }
    let guest_prog = Path::new(GUEST_PAYLOAD).join(name);
    let argv = [guest_prog.to_string_lossy().into_owned()]
        .into_iter()
        .chain(rest.iter().cloned())
        .collect();
    let paths = [("IGNORARIUM_IN", GUEST_IN), ("IGNORARIUM_OUT", GUEST_OUT)];
    let env = (job.env.iter().cloned())
        .chain(paths.map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    Ok(HostJob {
        argv,
        env,
        input_dir: job.input_dir.clone(),
        payload_dir: dir.to_path_buf(),
        output_dir: job.output_dir.clone(),
        seed: job.seed,
        wall_timeout: job.wall_timeout,
    })
}

/// The guest's self-test as the record level 1 writes, so the `sandbox.selftest` event and
/// its readers stay as they are. The guest's `io_uring_setup` reads as level 1's does:
/// denied, absent or open.
pub fn selftest_of(t: &proto::SelfTest) -> SelfTest {
    let io_uring = Call::of(t.io_uring_errno);
    SelfTest {
        connect_blocked: t.only_loopback && t.route_blocked,
        vsock_blocked: t.host_path_blocked,
        io_uring_blocked: io_uring != Call::Open,
        vsock_absent: false,
        io_uring_absent: io_uring == Call::Absent,
    }
}

/// The row's outcome from what `host::run_row` returned, `wall_seconds` after the row
/// started, for a machine of `cpus` vCPUs. A failed self-test never ran the payload. Without
/// a report the payload's CPU is unknown, unless the VMM never started, the guest never
/// spoke, or the box stopped the row. A report no guest can mean (more CPU than the row had,
/// an exit code outside 0 to 255, a signal outside 1 to 127) is a fault, not data. The box
/// ended the row when the guest hit the wall timeout, the watch said stop, or the guest missed
/// the host's deadline. The output links come with the hash that covers them.
pub fn outcome_of(res: Result<VmOutcome, VmError>, wall_seconds: f64, cpus: u32) -> RowOutcome {
    let o = match res {
        Ok(o) => o,
        Err(e) => {
            let cpu_unknown = match &e {
                VmError::Boot(_) | VmError::Stopped => None,
                VmError::Io(io) if io.kind() == io::ErrorKind::InvalidInput => None,
                _ => Some(NO_REPORT.to_string()),
            };
            let ended = matches!(e, VmError::Stopped | VmError::Timeout(_));
            let exit = match e {
                VmError::Stopped => Exit::Stopped,
                e => Exit::Infra(e.to_string()),
            };
            return RowOutcome {
                cpu_unknown,
                ended,
                ..RowOutcome::failed(exit, wall_seconds)
            };
        }
    };
    let r = o.report;
    let selftest = selftest_of(&r.selftest);
    let most = wall_seconds * f64::from(cpus);
    if !(0.0..=most).contains(&r.cpu_seconds) {
        let why = format!(
            "the guest reported {} CPU seconds, outside 0 to {most} for {cpus} vCPUs over {wall_seconds} s",
            r.cpu_seconds
        );
        return RowOutcome {
            selftest: Some(selftest),
            cpu_unknown: Some(BAD_CPU.into()),
            ..RowOutcome::failed(Exit::Infra(why), wall_seconds)
        };
    }
    let ended = r.end == Some(End::Timeout);
    let exit = if !selftest.ok() {
        Exit::Infra(format!(
            "guest self-test failed: {} was not blocked",
            selftest.failed().join(" and ")
        ))
    } else {
        let reported = |what: String| Exit::Infra(format!("the guest reported {what}"));
        match (r.end, r.pack_refused.as_deref()) {
            (None, _) => Exit::Infra("the guest ran no payload".into()),
            (Some(End::Code(c)), _) if !(0..=255).contains(&c) => {
                reported(format!("exit code {c}"))
            }
            (Some(End::Signal(s)), _) if !(1..=127).contains(&s) => reported(format!("signal {s}")),
            (Some(End::Code(0)), Some(why)) => Exit::Void(format!("cannot pack output: {why}")),
            (Some(End::Code(0)), None) => Exit::Ok,
            (Some(End::Code(c)), _) => Exit::Payload(c),
            (Some(End::Signal(s)), _) => Exit::Payload(128 + s),
            (Some(End::Timeout), _) => Exit::Payload(TIMEOUT_CODE),
        }
    };
    let hashed = matches!(exit, Exit::Ok | Exit::Payload(_)) && r.pack_refused.is_none();
    let (output_hash, output_links) = if hashed {
        let links = o.links.iter();
        let rows = links.map(|l| tree::link_row(&l.path, &l.target)).collect();
        (o.output_hash, rows)
    } else {
        (String::new(), Vec::new())
    };
    RowOutcome {
        exit,
        cpu_seconds: r.cpu_seconds,
        wall_seconds,
        output_hash,
        selftest: Some(selftest),
        cpu_unknown: None,
        ended,
        output_links,
        box_cpu_seconds: 0.0,
    }
}

/// The rootfs disk of the environment at `root`: `<root>.ext4`, built on the first call. A
/// published root never changes, so the disk never goes stale. `Ok(Err(why))` when the image
/// holds a file mkfs.ext4 cannot read: it copies each file as the box's user, and the root
/// keeps the image's modes. A failed build leaves nothing beside the root.
pub fn rootfs_disk(root: &Path) -> io::Result<Result<PathBuf, String>> {
    let mut disk = root.as_os_str().to_owned();
    disk.push(".ext4");
    let disk = PathBuf::from(disk);
    if disk.is_file() {
        return Ok(Ok(disk));
    }
    let tmp = temp_beside(&disk)?;
    let built = image::unreadable(root).and_then(|found| match found {
        Some(file) => Ok(Err(format!("{file}, so mkfs.ext4 cannot read it"))),
        None => {
            image::ext4(Some(root), &tmp, image::size_mib(root)?)?;
            fs::rename(&tmp, &disk).map(|()| Ok(disk))
        }
    });
    if !matches!(built, Ok(Ok(_))) {
        let _ = fs::remove_file(&tmp);
    }
    built
}

/// A row's scratch folder, `<temp>/ignorarium-vm-<pid>-<n>` (mode 0700): the initramfs and
/// an empty `work/`. It goes, with all it holds, when the row ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(initramfs: &[u8]) -> io::Result<Scratch> {
        let temp = std::env::temp_dir();
        sweep(&temp);
        let n = ROWS.fetch_add(1, Ordering::Relaxed);
        let dir = temp.join(format!("{SCRATCH}{}-{n}", std::process::id()));
        fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let scratch = Scratch(dir);
        fs::write(scratch.0.join("initramfs.cpio"), initramfs)?;
        fs::create_dir(scratch.0.join("work"))?;
        Ok(scratch)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Removes the scratch folders that boxes left when they died mid-row: this user's
/// `ignorarium-vm-<pid>-<n>` folders whose pid no process has. A pid in use, by any process,
/// keeps its folders.
fn sweep(temp: &Path) {
    let Ok(entries) = fs::read_dir(temp) else {
        return;
    };
    let me = geteuid().as_raw();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = name
            .to_str()
            .and_then(|n| n.strip_prefix(SCRATCH)?.split_once('-'))
            .and_then(|(pid, _)| pid.parse::<i32>().ok())
            .and_then(Pid::from_raw);
        // Only a scratch name costs a stat: the temp folder can hold many other entries.
        if let Some(pid) = pid
            && entry.metadata().is_ok_and(|m| m.is_dir() && m.uid() == me)
            && test_kill_process(pid) == Err(Errno::SRCH)
        {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}
