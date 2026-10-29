//! Level 1: each row runs in a bubblewrap sandbox. It gets new user, mount, PID, network,
//! IPC, UTS and cgroup namespaces, an empty read-only root with read-only binds of the
//! environment, the input folder and the payload, one writable output folder, and a
//! seccomp filter. See the bubblewrap manual: <https://github.com/containers/bubblewrap>.
use super::{Backend, Exit, Probe, RowJob, RowOutcome, TIMEOUT_CODE, Watch, outcome, supervise};
use crate::sys;
use rustix::pipe::{PipeFlags, pipe_with};
use rustix::process::{getpid, set_child_subreaper};
use std::cell::Cell;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

/// The first bubblewrap with `--disable-userns`.
const FLOOR: (u32, u32) = (0, 8);
/// The environment root's entries bound into the sandbox, each when present.
const ROOT_ENTRIES: [&str; 5] = ["usr", "bin", "lib", "lib64", "etc"];
/// Bytes kept from the end of bubblewrap's stderr and status pipe.
const TAIL: usize = 4096;
/// Time the probe's test sandbox may take.
const SMOKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Time to wait for the pipes to reach their end once bubblewrap is reaped.
const DRAIN_GRACE: Duration = Duration::from_secs(5);

/// The level 1 backend. Its first sandbox makes the calling process a child subreaper
/// for the rest of its life: bubblewrap leaves the sandbox's PID 1 unreaped, and the box
/// must reap it to count the payload's CPU. Every orphan of the process's descendants
/// then becomes its child, and stays a zombie unless something reaps it.
pub struct Level1 {
    /// The bubblewrap binary.
    pub bwrap: PathBuf,
    /// The box binary, which the self-test sandbox runs.
    pub exe: PathBuf,
}

impl Default for Level1 {
    /// `bwrap` from `PATH`, or the bare name when it is absent so the probe names it, and
    /// this binary.
    fn default() -> Self {
        let bwrap = std::env::var_os("PATH")
            .and_then(|p| {
                std::env::split_paths(&p)
                    .map(|d| d.join("bwrap"))
                    .find(|f| f.is_file())
            })
            .unwrap_or_else(|| "bwrap".into());
        Level1::with(bwrap, std::env::current_exe().unwrap_or_default())
    }
}

impl Level1 {
    pub fn new() -> Level1 {
        Level1::default()
    }

    pub fn with(bwrap: PathBuf, exe: PathBuf) -> Level1 {
        Level1 { bwrap, exe }
    }

    /// The version `bwrap --version` prints, as text and as (major, minor).
    fn version(&self) -> Result<(String, (u32, u32)), String> {
        let out = Command::new(&self.bwrap)
            .arg("--version")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| format!("cannot run {}: {e}", self.bwrap.display()))?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let number = |p: Option<&str>| p?.parse::<u32>().ok();
        text.strip_prefix("bubblewrap ")
            .and_then(|v| {
                let mut parts = v.split('.');
                Some((
                    v.to_string(),
                    (number(parts.next())?, number(parts.next())?),
                ))
            })
            .ok_or_else(|| {
                format!(
                    "{} --version printed {text:?}, not a version",
                    self.bwrap.display()
                )
            })
    }

    /// Why level 1 cannot run here, if it cannot: bubblewrap is missing or older than the
    /// floor, or a test sandbox on the host root fails.
    fn fault(&self) -> Option<String> {
        let (text, number) = match self.version() {
            Ok(v) => v,
            Err(why) => return Some(why),
        };
        if number < FLOOR {
            return Some(format!(
                "bubblewrap {text} is older than 0.8, which level 1 needs for --disable-userns"
            ));
        }
        let smoke = Sandbox {
            mounts: root_mounts(Path::new("/")),
            run: os_args(["--", "/bin/true"]),
        };
        let job = RowJob {
            wall_timeout: SMOKE_TIMEOUT,
            ..RowJob::default()
        };
        match self.launch(&smoke, &job, Instant::now(), &|| true).0 {
            Exit::Ok => None,
            Exit::Infra(why) => Some(why),
            other => Some(format!("the test sandbox ended with {other:?}")),
        }
    }

    /// Run one sandbox under `supervise`. An exit code on bubblewrap's status pipe means
    /// the command ran; none means bubblewrap failed first, and the fault carries the end
    /// of its stderr. A stop, the wall timeout and a fault of the box's own stand as they
    /// are. The CPU count includes the sandbox's PID 1 and every process it reaped.
    fn launch(
        &self,
        sandbox: &Sandbox,
        job: &RowJob,
        start: Instant,
        watch: &dyn Watch,
    ) -> (Exit, f64) {
        // bubblewrap exits without reaping the sandbox's PID 1, so its CPU time, and the
        // payload's, never reach bubblewrap's own count. As a subreaper the box inherits
        // that PID 1 and reaps it below. A sandbox the box kills still loses its count:
        // the kernel then reaps its processes with SIGCHLD ignored, which counts nothing.
        if let Err(e) = set_child_subreaper(Some(getpid())) {
            return (Exit::Infra(format!("cannot become a subreaper: {e}")), 0.0);
        }
        let pipes = match Pipes::new() {
            Ok(p) => p,
            Err(e) => {
                return (
                    Exit::Infra(format!("cannot make the sandbox pipes: {e}")),
                    0.0,
                );
            }
        };
        let inherited = [pipes.filter.as_raw_fd(), pipes.status_w.as_raw_fd()];
        let mut cmd = Command::new(&self.bwrap);
        cmd.args(&sandbox.mounts)
            .args(isolation_args(inherited[0], inherited[1]))
            .args(&sandbox.run)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(pipes.stderr_w));
        sys::inherit_only(&mut cmd, inherited);
        let started = Started {
            held: Cell::new(Some((pipes.filter, pipes.status_w))),
            inner: watch,
        };
        let (exit, cpu) = supervise(cmd, job, start, &started);
        // Closes the inherited ends when bubblewrap never started.
        drop(started);
        let status = pipes.status.recv_timeout(DRAIN_GRACE).unwrap_or_default();
        let stderr = pipes.stderr.recv_timeout(DRAIN_GRACE).unwrap_or_default();
        let cpu = cpu + field(&status, "child-pid").map_or(0.0, reap);
        // `Payload(TIMEOUT_CODE)` without an exit document is the wall timeout; with one,
        // bubblewrap passed on the payload's own 124, which the document holds too.
        if matches!(
            exit,
            Exit::Stopped | Exit::Infra(_) | Exit::Payload(TIMEOUT_CODE)
        ) {
            return (exit, cpu);
        }
        let exit = match field(&status, "exit-code") {
            Some(0) => Exit::Ok,
            Some(c) => Exit::Payload(c),
            None => Exit::Infra(format!(
                "bubblewrap reported no payload exit: {}",
                stderr.trim()
            )),
        };
        (exit, cpu)
    }
}

impl Backend for Level1 {
    fn level(&self) -> u8 {
        1
    }

    fn probe(&self) -> Probe {
        match self.fault() {
            None => Probe::Available,
            Some(why) => Probe::Unavailable(why),
        }
    }

    fn launcher(&self) -> String {
        match self.version() {
            Ok((text, _)) => format!("bubblewrap {text}"),
            Err(_) => "bubblewrap unknown".into(),
        }
    }

    fn run_row(&self, job: &RowJob, watch: &dyn Watch) -> io::Result<RowOutcome> {
        let start = Instant::now();
        let (exit, cpu_seconds) = match row_args(job) {
            Ok(sandbox) => self.launch(&sandbox, job, start, watch),
            Err(why) => (Exit::Infra(why), 0.0),
        };
        Ok(outcome(job, start, exit, cpu_seconds))
    }
}

/// One sandbox's own bubblewrap arguments, around the isolation arguments. bubblewrap
/// applies mounts in order and cannot add one under a read-only root, so `mounts` go
/// before the isolation arguments, which end with `--remount-ro /`. `--clearenv` drops
/// every variable set before it, so `run` (environment, working folder, `--` and the
/// command) goes after them.
pub struct Sandbox {
    pub mounts: Vec<OsString>,
    pub run: Vec<OsString>,
}

/// The isolation every level 1 sandbox gets. `seccomp_fd` is read for the filter;
/// `status_fd` receives bubblewrap's JSON status, one document per line.
pub fn isolation_args(seccomp_fd: i32, status_fd: i32) -> Vec<OsString> {
    let (seccomp_fd, status_fd) = (seccomp_fd.to_string(), status_fd.to_string());
    os_args([
        "--unshare-user",
        "--unshare-ipc",
        "--unshare-pid",
        "--unshare-net",
        "--unshare-uts",
        "--unshare-cgroup",
        "--disable-userns",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
        "--seccomp",
        &seccomp_fd,
        "--json-status-fd",
        &status_fd,
        "--proc",
        "/proc",
        "--dir",
        "/dev",
        "--dev-bind",
        "/dev/null",
        "/dev/null",
        "--dev-bind",
        "/dev/zero",
        "/dev/zero",
        "--dev-bind",
        "/dev/urandom",
        "/dev/urandom",
        "--remount-ro",
        "/",
    ])
}

/// A row's sandbox: the environment root (the host's `/` when `job.rootfs` is `None`),
/// `input_dir` at `/in` read-only, `output_dir` at `/out` writable, and the folder of
/// `argv[0]` at `/payload` read-only. The payload starts in `/out` as
/// `/payload/<file name>`. A missing `input_dir` leaves `/in` absent, as level 0 hands
/// the payload a path that does not exist.
pub fn row_args(job: &RowJob) -> Result<Sandbox, String> {
    let (prog, rest) = job.argv.split_first().ok_or("empty argv")?;
    let prog = Path::new(prog);
    let (Some(dir), Some(name)) = (prog.parent(), prog.file_name()) else {
        return Err(format!("payload {} names no file", prog.display()));
    };
    let root = job.rootfs.as_deref().unwrap_or(Path::new("/"));
    if !root.is_dir() {
        return Err(format!(
            "environment root {} is not a folder",
            root.display()
        ));
    }
    let mut mounts = root_mounts(root);
    mounts.extend(op("--ro-bind-try", &job.input_dir, "/in"));
    mounts.extend(op("--bind", &job.output_dir, "/out"));
    mounts.extend(op("--ro-bind", dir, "/payload"));
    let paths = [("IGNORARIUM_IN", "/in"), ("IGNORARIUM_OUT", "/out")];
    let mut run = Vec::new();
    for (k, v) in job
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .chain(paths)
    {
        run.extend(op("--setenv", k, v));
    }
    run.extend(os_args(["--chdir", "/out", "--"]));
    run.push(Path::new("/payload").join(name).into());
    run.extend(os_args(rest));
    Ok(Sandbox { mounts, run })
}

/// A read-only bind for each directory of `ROOT_ENTRIES` in `root`, and the same link for
/// each symbolic link; an absent entry adds nothing.
fn root_mounts(root: &Path) -> Vec<OsString> {
    let mut mounts = Vec::new();
    for entry in ROOT_ENTRIES {
        let (path, at) = (root.join(entry), Path::new("/").join(entry));
        match std::fs::read_link(&path) {
            Ok(target) => mounts.extend(op("--symlink", target, at)),
            Err(_) if path.is_dir() => mounts.extend(op("--ro-bind", path, at)),
            Err(_) => {}
        }
    }
    mounts
}

fn os_args<S: AsRef<OsStr>>(items: impl IntoIterator<Item = S>) -> Vec<OsString> {
    items.into_iter().map(|s| s.as_ref().to_owned()).collect()
}

/// A bubblewrap option with a source and a destination.
fn op(flag: &str, from: impl AsRef<OsStr>, to: impl AsRef<OsStr>) -> [OsString; 3] {
    [flag.into(), from.as_ref().into(), to.as_ref().into()]
}

/// Field `key` of bubblewrap's status documents, one JSON object per line.
fn field<T: TryFrom<i64>>(status: &str, key: &str) -> Option<T> {
    status
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find_map(|v| v[key].as_i64())
        .and_then(|n| T::try_from(n).ok())
}

/// Reap `pid` if it is the box's child, and return the CPU seconds it and the processes it
/// reaped used; 0 when it is not. The sandbox's PID 1 dies with bubblewrap
/// (`--die-with-parent`), so the wait is short.
fn reap(pid: u32) -> f64 {
    if sys::has_exited(pid).is_err() {
        return 0.0;
    }
    match sys::wait4(pid, true) {
        Ok(Some(r)) => r.cpu_seconds,
        _ => 0.0,
    }
}

/// The pipes of one sandbox, all close-on-exec in the box. bubblewrap alone inherits
/// `filter` (the seccomp program, already written) and `status_w`: `sys::inherit_only`
/// clears close-on-exec on them in its child only.
struct Pipes {
    filter: OwnedFd,
    status_w: OwnedFd,
    stderr_w: OwnedFd,
    status: Receiver<String>,
    stderr: Receiver<String>,
}

impl Pipes {
    fn new() -> io::Result<Pipes> {
        let (filter, filter_w) = pipe_with(PipeFlags::CLOEXEC)?;
        File::from(filter_w).write_all(&super::seccomp::program())?;
        let (status_r, status_w) = pipe_with(PipeFlags::CLOEXEC)?;
        let (stderr_r, stderr_w) = pipe_with(PipeFlags::CLOEXEC)?;
        Ok(Pipes {
            filter,
            status_w,
            stderr_w,
            status: drain(status_r)?,
            stderr: drain(stderr_r)?,
        })
    }
}

/// Read `fd` to its end on a thread; the receiver gets the last `TAIL` bytes as text.
fn drain(fd: OwnedFd) -> io::Result<Receiver<String>> {
    let (tx, rx) = channel();
    std::thread::Builder::new().spawn(move || {
        let mut f = File::from(fd);
        let (mut kept, mut buf) = (Vec::new(), [0u8; TAIL]);
        loop {
            match f.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    kept.extend_from_slice(&buf[..n]);
                    kept.drain(..kept.len().saturating_sub(TAIL));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = tx.send(String::from_utf8_lossy(&kept).into_owned());
    })?;
    Ok(rx)
}

/// Forwards to `inner`. On `started` it first closes the box's copies of the ends
/// bubblewrap inherited, so the status pipe ends when bubblewrap's copy closes.
struct Started<'a> {
    held: Cell<Option<(OwnedFd, OwnedFd)>>,
    inner: &'a dyn Watch,
}

impl Watch for Started<'_> {
    fn tick(&self) -> bool {
        self.inner.tick()
    }

    fn started(&self, pid: u32) {
        drop(self.held.take());
        self.inner.started(pid);
    }
}
