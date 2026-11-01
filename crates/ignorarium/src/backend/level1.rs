//! Level 1: each row runs in a bubblewrap sandbox. It gets new user, mount, PID, network,
//! IPC, UTS and cgroup namespaces, an empty read-only root with read-only binds of the
//! environment, the input folder and the payload, one writable output folder, and a
//! seccomp filter. See the bubblewrap manual: <https://github.com/containers/bubblewrap>.
use super::rootfs::{self, KEPT};
use super::selftest::SelfTest;
use super::{
    Backend, Exit, Probe, RowJob, RowOutcome, TIMEOUT_CODE, Watch, host_cpus, outcome, supervise,
};
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
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

/// A bubblewrap version as text and as (major, minor).
type Version = (String, (u32, u32));
/// The first bubblewrap with `--disable-userns`.
const FLOOR: (u32, u32) = (0, 8);
/// Bytes kept from the end of bubblewrap's stderr and status pipe.
const TAIL: usize = 4096;
/// Time the probe's test sandbox may take.
const SMOKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Time the self-test sandbox may take, within the row's own wall time.
const SELFTEST_LIMIT: Duration = Duration::from_secs(10);
/// Time to wait for the pipes to reach their end once bubblewrap is reaped.
const DRAIN_GRACE: Duration = Duration::from_secs(5);
/// Time to wait for the sandbox's PID 1 to become reapable once bubblewrap is gone. A
/// payload stuck in uninterruptible sleep outlasts a kill, and must not hold the box.
const REAP_LIMIT: Duration = Duration::from_secs(5);
const REAP_POLL: Duration = Duration::from_millis(5);
/// Where the box binary appears in the self-test sandbox.
const BOX_PATH: &str = "/box/ignorarium";
/// Where binfmt_misc handlers are registered.
const BINFMT: &str = "/proc/sys/fs/binfmt_misc";
/// The name WSL gives its handlers for Windows programs, as a prefix: `WSLInterop`,
/// `WSLInterop-late`.
const INTEROP: &str = "WSLInterop";

/// The level 1 backend. Its first sandbox makes the calling process a child subreaper
/// for the rest of its life: bubblewrap leaves the sandbox's PID 1 unreaped, and the box
/// must reap it to count the payload's CPU. Every orphan of the process's descendants
/// then becomes its child, and stays a zombie unless something reaps it.
pub struct Level1 {
    /// The bubblewrap binary.
    pub bwrap: PathBuf,
    /// The box binary, which the self-test sandbox runs.
    pub exe: PathBuf,
    /// Marks descriptors close-on-exec from the box before it starts bubblewrap. A field
    /// so a test can stand in a kernel that lacks the call.
    close_range: fn() -> io::Result<()>,
    /// Where the probe reads binfmt_misc, for WSL's interop handlers.
    binfmt: Binfmt,
    /// `bwrap --version`, read on first use: every row record names the launcher.
    version: OnceLock<Result<Version, String>>,
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
        Level1 {
            bwrap,
            exe,
            close_range: || sys::close_range_cloexec(libc::c_uint::MAX, libc::c_uint::MAX),
            binfmt: Binfmt::host(),
            version: OnceLock::new(),
        }
    }

    /// The version `bwrap --version` prints, as text and as (major, minor).
    fn version(&self) -> &Result<Version, String> {
        self.version.get_or_init(|| self.read_version())
    }

    fn read_version(&self) -> Result<Version, String> {
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

    /// Why level 1 cannot run here, if it cannot: the kernel lacks a call, WSL interop reaches
    /// out of a sandbox, bubblewrap is missing or older than the floor, or a test sandbox on
    /// the host root fails.
    fn fault(&self) -> Option<String> {
        // `sys::inherit_only` needs this call in every spawn. Test it here, so a kernel
        // without it is named, not blamed on bubblewrap's "Invalid argument".
        if let Err(e) = (self.close_range)() {
            return Some(format!(
                "level 1 needs Linux 5.11 or later (close_range with CLOSE_RANGE_CLOEXEC): {e}"
            ));
        }
        if let Some(why) = interop_fault(self.binfmt) {
            return Some(why);
        }
        let (text, number) = match self.version() {
            Ok((text, number)) => (text, *number),
            Err(why) => return Some(why.clone()),
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
        match self
            .launch(&smoke, &job, Instant::now(), &|| true, false)
            .exit
        {
            Exit::Ok => None,
            Exit::Infra(why) => Some(why),
            other => Some(format!("the test sandbox ended with {other:?}")),
        }
    }

    /// Run one sandbox under `supervise`. An exit code on bubblewrap's status pipe means
    /// the command ran; none means bubblewrap failed first, and the fault carries the end
    /// of its stderr. A stop, the wall timeout and a fault of the box's own stand as they
    /// are. The CPU count includes the sandbox's PID 1 and every process it reaped. With
    /// `capture`, the command's standard output comes back in `stdout`; otherwise it goes
    /// to nowhere.
    fn launch(
        &self,
        sandbox: &Sandbox,
        job: &RowJob,
        start: Instant,
        watch: &dyn Watch,
        capture: bool,
    ) -> Launched {
        // bubblewrap exits without reaping the sandbox's PID 1, so its CPU time, and the
        // payload's, never reach bubblewrap's own count. As a subreaper the box inherits
        // that PID 1 and reaps it below. A sandbox the box kills still loses its count:
        // the kernel then reaps its processes with SIGCHLD ignored, which counts nothing.
        if let Err(e) = set_child_subreaper(Some(getpid())) {
            return Launched::infra(format!("cannot become a subreaper: {e}"));
        }
        let mut pipes = match Pipes::new(capture) {
            Ok(p) => p,
            Err(e) => return Launched::infra(format!("cannot make the sandbox pipes: {e}")),
        };
        let inherited = [pipes.filter.as_raw_fd(), pipes.status_w.as_raw_fd()];
        let mut cmd = Command::new(&self.bwrap);
        cmd.args(&sandbox.mounts)
            .args(isolation_args(inherited[0], inherited[1]))
            .args(&sandbox.run)
            .stdin(Stdio::null())
            .stdout(pipes.stdout_w.take().map_or_else(Stdio::null, Stdio::from))
            .stderr(Stdio::from(pipes.stderr_w));
        sys::inherit_only(&mut cmd, inherited);
        // bubblewrap runs under the cap too; it starts within 256 KiB, below the 1 MiB floor.
        sys::limit_data(&mut cmd, job.data_limit());
        let started = Started {
            held: Cell::new(Some((pipes.filter, pipes.status_w))),
            inner: watch,
        };
        let ran = supervise(cmd, job, start, &started);
        let (exit, cpu, ended) = (ran.exit, ran.cpu_seconds, ran.ended);
        // Closes the inherited ends when bubblewrap never started.
        drop(started);
        let status = pipes.status.recv_timeout(DRAIN_GRACE).unwrap_or_default();
        let stderr = pipes.stderr.recv_timeout(DRAIN_GRACE).unwrap_or_default();
        let stdout = pipes
            .stdout
            .and_then(|r| r.recv_timeout(DRAIN_GRACE).ok())
            .unwrap_or_default();
        let (cpu, cpu_unknown) = match field(&status, "child-pid").map(|p| reap(p, REAP_LIMIT)) {
            None => (cpu, None),
            Some(Some(c)) => (cpu + c, None),
            Some(None) => (
                cpu,
                Some("the sandbox's PID 1 did not exit in time; its CPU time is unknown".into()),
            ),
        };
        // `Payload(TIMEOUT_CODE)` without an exit document is the wall timeout; with one,
        // bubblewrap passed on the payload's own 124, which the document holds too.
        let exit = if matches!(
            exit,
            Exit::Stopped | Exit::Infra(_) | Exit::Payload(TIMEOUT_CODE)
        ) {
            exit
        } else {
            match field(&status, "exit-code") {
                Some(0) => Exit::Ok,
                Some(c) => Exit::Payload(c),
                None => Exit::Infra(format!(
                    "bubblewrap reported no payload exit: {}",
                    stderr.trim()
                )),
            }
        };
        Launched {
            exit,
            cpu,
            cpu_unknown,
            stdout,
            ended,
        }
    }

    /// The self-test sandbox: the host root, the box binary and the isolation of a row.
    /// `result` is what the self-test found, or the row's exit when the test failed or
    /// could not run, with what it found when it printed a readable result. Its CPU and
    /// its unknown-CPU note go on the row.
    fn self_test(&self, job: &RowJob, start: Instant, watch: &dyn Watch) -> Checked {
        let mut mounts = root_mounts(Path::new("/"));
        mounts.extend(op("--ro-bind", &self.exe, BOX_PATH));
        let sandbox = Sandbox {
            mounts,
            run: os_args(["--", BOX_PATH, "dev", "selftest"]),
        };
        let (limit, row_first) = self_test_limit(start.elapsed(), job.wall_timeout, SELFTEST_LIMIT);
        let probe_job = RowJob {
            wall_timeout: limit,
            ..RowJob::default()
        };
        let l = self.launch(&sandbox, &probe_job, start, watch, true);
        let found = serde_json::from_str::<SelfTest>(l.stdout.trim()).ok();
        let result = match (&found, &l.exit) {
            (Some(t), Exit::Ok) if t.ok() => Ok(*t),
            (_, Exit::Stopped) => Err((Exit::Stopped, None)),
            // The row's own wall time ran out during the self-test: the row timed out.
            (_, Exit::Payload(TIMEOUT_CODE)) if row_first => {
                Err((Exit::Payload(TIMEOUT_CODE), found))
            }
            (Some(t), _) if !t.ok() => Err((
                Exit::Infra(format!(
                    "sandbox self-test failed: {} was not blocked",
                    t.failed().join(" and ")
                )),
                found,
            )),
            _ => Err((
                Exit::Infra(format!(
                    "sandbox self-test failed: it ended with {:?}: {}",
                    l.exit,
                    l.stdout.trim()
                )),
                found,
            )),
        };
        Checked {
            result,
            cpu: l.cpu,
            cpu_unknown: l.cpu_unknown,
            ended: l.ended,
        }
    }
}

/// How the probe reads binfmt_misc: the entry names, and one entry's text. Fields, so a test
/// can stand in a host.
#[derive(Clone, Copy)]
struct Binfmt {
    list: fn() -> io::Result<Vec<String>>,
    read: fn(&str) -> io::Result<String>,
}

impl Binfmt {
    /// The host's.
    fn host() -> Binfmt {
        Binfmt {
            list: || names_in(Path::new(BINFMT)),
            read: |name| std::fs::read_to_string(Path::new(BINFMT).join(name)),
        }
    }
}

/// The names in the folder `dir`; none when it does not exist, as binfmt_misc may not be.
fn names_in(dir: &Path) -> io::Result<Vec<String>> {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .map(|e| Ok(e?.file_name().to_string_lossy().into_owned()))
            .collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Why WSL interop makes level 1 unsafe, if it does: an enabled interop handler with the `F`
/// flag, or one the probe cannot read. With `F` the kernel opened the interpreter when the
/// handler was registered, so a program in the sandbox could start Windows programs outside
/// it. Without `F` (WSL's default `P`), the interpreter is looked up in the sandbox's root,
/// where it is not. Other handlers run their interpreter inside the sandbox.
fn interop_fault(binfmt: Binfmt) -> Option<String> {
    let names = match (binfmt.list)() {
        Ok(names) => names,
        Err(e) => return Some(format!("cannot list {BINFMT}: {e}")),
    };
    names
        .iter()
        .filter(|n| n.starts_with(INTEROP))
        .find_map(|name| {
            let text = match (binfmt.read)(name) {
                Ok(text) => text,
                Err(e) => return Some(format!("cannot read {BINFMT}/{name}: {e}")),
            };
            let enabled = text.lines().next() == Some("enabled");
            let flags = text.lines().find_map(|l| l.strip_prefix("flags:"));
            (enabled && flags.is_some_and(|f| f.contains('F'))).then(|| {
                format!(
                    "{BINFMT}/{name} has the F flag, so a program in the sandbox could start \
                 Windows programs outside it"
                )
            })
        })
}

/// The self-test's outcome and what it cost.
struct Checked {
    result: Result<SelfTest, (Exit, Option<SelfTest>)>,
    cpu: f64,
    cpu_unknown: Option<String>,
    /// The box killed the self-test sandbox.
    ended: bool,
}

/// The wall limit for the self-test sandbox, on the row's clock: `own` after `elapsed`,
/// but never past the row's `wall`. The flag is true when the row's time ends first, so a
/// timeout is the row's and not a fault of the self-test.
fn self_test_limit(elapsed: Duration, wall: Duration, own: Duration) -> (Duration, bool) {
    let own_end = elapsed + own;
    (own_end.min(wall), wall <= own_end)
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

    fn cpus(&self) -> u32 {
        host_cpus()
    }

    fn launcher(&self) -> String {
        match self.version() {
            Ok((text, _)) => format!("bubblewrap {text}"),
            Err(_) => "bubblewrap unknown".into(),
        }
    }

    fn run_row(&self, job: &RowJob, watch: &dyn Watch) -> io::Result<RowOutcome> {
        let start = Instant::now();
        let sandbox = match row_args(job) {
            Ok(s) => s,
            Err(why) => return Ok(outcome(job, start, Exit::Infra(why), 0.0, false)),
        };
        let checked = self.self_test(job, start, watch);
        let selftest = match checked.result {
            Ok(t) => Some(t),
            Err((exit, found)) => {
                return Ok(RowOutcome {
                    selftest: found,
                    cpu_unknown: checked.cpu_unknown,
                    ..outcome(job, start, exit, checked.cpu, checked.ended)
                });
            }
        };
        let l = self.launch(&sandbox, job, start, watch, false);
        Ok(RowOutcome {
            selftest,
            cpu_unknown: l.cpu_unknown.or(checked.cpu_unknown),
            ..outcome(job, start, l.exit, checked.cpu + l.cpu, l.ended)
        })
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
            "the root of environment {} is not a folder",
            rootfs::name_of(root)
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

/// A read-only bind for each directory of `KEPT` in `root`, and the same link for
/// each symbolic link; an absent entry adds nothing.
fn root_mounts(root: &Path) -> Vec<OsString> {
    let mut mounts = Vec::new();
    for entry in KEPT {
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
/// reaped used; 0 when it is not the box's child. The sandbox's PID 1 dies with bubblewrap
/// (`--die-with-parent`), so the wait is short. `None` when it has not exited after
/// `limit`; the box, a subreaper, collects it later.
fn reap(pid: u32, limit: Duration) -> Option<f64> {
    let deadline = Instant::now() + limit;
    loop {
        match sys::has_exited(pid) {
            Err(_) => return Some(0.0),
            Ok(true) => {
                return Some(sys::wait4(pid).map_or(0.0, |r| r.cpu_seconds));
            }
            Ok(false) if Instant::now() < deadline => std::thread::sleep(REAP_POLL),
            Ok(false) => return None,
        }
    }
}

/// How one `launch` ended.
struct Launched {
    exit: Exit,
    cpu: f64,
    cpu_unknown: Option<String>,
    stdout: String,
    /// The box killed the sandbox.
    ended: bool,
}

impl Launched {
    fn infra(why: String) -> Launched {
        Launched {
            exit: Exit::Infra(why),
            cpu: 0.0,
            cpu_unknown: None,
            stdout: String::new(),
            ended: false,
        }
    }
}

/// The pipes of one sandbox, all close-on-exec in the box. bubblewrap alone inherits
/// `filter` (the seccomp program, already written) and `status_w`: `sys::inherit_only`
/// clears close-on-exec on them in its child only.
struct Pipes {
    filter: OwnedFd,
    status_w: OwnedFd,
    stderr_w: OwnedFd,
    /// Present only when the command's standard output is wanted.
    stdout_w: Option<OwnedFd>,
    status: Receiver<String>,
    stderr: Receiver<String>,
    stdout: Option<Receiver<String>>,
}

impl Pipes {
    fn new(capture: bool) -> io::Result<Pipes> {
        let (filter, filter_w) = pipe_with(PipeFlags::CLOEXEC)?;
        File::from(filter_w).write_all(&super::seccomp::program())?;
        let (status_r, status_w) = pipe_with(PipeFlags::CLOEXEC)?;
        let (stderr_r, stderr_w) = pipe_with(PipeFlags::CLOEXEC)?;
        let stdout = if capture {
            let (r, w) = pipe_with(PipeFlags::CLOEXEC)?;
            Some((w, drain(r)?))
        } else {
            None
        };
        let (stdout_w, stdout) = stdout.unzip();
        Ok(Pipes {
            filter,
            status_w,
            stderr_w,
            stdout_w,
            status: drain(status_r)?,
            stderr: drain(stderr_r)?,
            stdout,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_self_test_limit_is_its_own_unless_the_row_ends_first() {
        let sec = Duration::from_secs;
        // Plenty of row time left: the self-test's own limit, after the time already used.
        assert_eq!(self_test_limit(sec(2), sec(100), sec(10)), (sec(12), false));
        // The row's time ends first.
        assert_eq!(self_test_limit(sec(2), sec(5), sec(10)), (sec(5), true));
        // Both end together: the row's time is the one that ran out.
        assert_eq!(self_test_limit(sec(2), sec(12), sec(10)), (sec(12), true));
        assert_eq!(self_test_limit(sec(2), sec(13), sec(10)), (sec(12), false));
        // No time left at all.
        assert_eq!(self_test_limit(sec(9), sec(1), sec(10)), (sec(1), true));
    }

    #[test]
    fn a_kernel_without_close_range_cloexec_is_named_in_the_probe() {
        let mut b = Level1::with("/nonexistent/bwrap".into(), PathBuf::new());
        b.close_range = || Err(io::Error::from_raw_os_error(libc::EINVAL));
        assert_eq!(
            b.probe(),
            Probe::Unavailable(
                "level 1 needs Linux 5.11 or later (close_range with CLOSE_RANGE_CLOEXEC): \
                 Invalid argument (os error 22)"
                    .into()
            )
        );
        // The kernel check passes here, so the probe reaches bubblewrap and names it.
        let b = Level1::with("/nonexistent/bwrap".into(), PathBuf::new());
        let Probe::Unavailable(why) = b.probe() else {
            panic!("probed available")
        };
        assert!(why.starts_with("cannot run /nonexistent/bwrap: "), "{why}");
    }

    #[test]
    fn an_enabled_wsl_interop_handler_with_the_f_flag_makes_level_1_unavailable() {
        const P: &str = "enabled\ninterpreter /init\nflags: P\noffset 0\nmagic 4d5a\n";
        const PF: &str = "enabled\ninterpreter /init\nflags: PF\noffset 0\nmagic 4d5a\n";
        const OFF_F: &str = "disabled\ninterpreter /init\nflags: F\noffset 0\nmagic 4d5a\n";
        fn names(n: &[&str]) -> io::Result<Vec<String>> {
            Ok(n.iter().map(|s| s.to_string()).collect())
        }
        fn both() -> io::Result<Vec<String>> {
            names(&["register", "status", "WSLInterop", "WSLInterop-late"])
        }
        let p_only = Binfmt {
            list: || names(&["register", "status", "WSLInterop"]),
            read: |n| match n {
                "WSLInterop" => Ok(P.into()),
                _ => panic!("read {n}"),
            },
        };
        assert_eq!(interop_fault(p_only), None);
        let late_f = Binfmt {
            list: both,
            read: |n| match n {
                "WSLInterop" => Ok(P.into()),
                "WSLInterop-late" => Ok(PF.into()),
                _ => panic!("read {n}"),
            },
        };
        assert_eq!(
            interop_fault(late_f),
            Some(
                "/proc/sys/fs/binfmt_misc/WSLInterop-late has the F flag, so a program in the \
                 sandbox could start Windows programs outside it"
                    .into()
            )
        );
        let disabled_f = Binfmt {
            list: both,
            read: |n| match n {
                "WSLInterop" => Ok(P.into()),
                "WSLInterop-late" => Ok(OFF_F.into()),
                _ => panic!("read {n}"),
            },
        };
        assert_eq!(interop_fault(disabled_f), None);
        let unreadable = Binfmt {
            list: both,
            read: |n| match n {
                "WSLInterop" => Ok(P.into()),
                _ => Err(io::Error::from_raw_os_error(libc::EACCES)),
            },
        };
        assert_eq!(
            interop_fault(unreadable),
            Some(
                "cannot read /proc/sys/fs/binfmt_misc/WSLInterop-late: Permission denied \
                 (os error 13)"
                    .into()
            )
        );
        // Not mounted, or only other handlers: nothing to read, nothing to refuse.
        for list in [
            || names(&[]),
            || names(&["register", "status", "qemu-aarch64"]),
        ] {
            let read: fn(&str) -> io::Result<String> = |n| panic!("read {n}");
            assert_eq!(interop_fault(Binfmt { list, read }), None);
        }
        let unlisted = Binfmt {
            list: || Err(io::Error::from_raw_os_error(libc::EIO)),
            read: |n| panic!("read {n}"),
        };
        assert_eq!(
            interop_fault(unlisted),
            Some("cannot list /proc/sys/fs/binfmt_misc: Input/output error (os error 5)".into())
        );
        // The probe asks before it runs bubblewrap.
        let mut b = Level1::with("/nonexistent/bwrap".into(), PathBuf::new());
        b.binfmt = late_f;
        let Probe::Unavailable(why) = b.probe() else {
            panic!("probed available")
        };
        assert!(why.contains("WSLInterop-late has the F flag"), "{why}");
        b.binfmt = p_only;
        let Probe::Unavailable(why) = b.probe() else {
            panic!("probed available")
        };
        assert!(why.starts_with("cannot run /nonexistent/bwrap: "), "{why}");
    }

    #[test]
    fn names_in_lists_a_folder_and_nothing_for_a_missing_one() {
        let d = tempfile::TempDir::new().unwrap();
        std::fs::write(d.path().join("WSLInterop"), "").unwrap();
        std::fs::write(d.path().join("status"), "").unwrap();
        let mut got = names_in(d.path()).unwrap();
        got.sort();
        assert_eq!(got, ["WSLInterop", "status"]);
        assert_eq!(
            names_in(&d.path().join("absent")).unwrap(),
            Vec::<String>::new()
        );
        // A path that is not a folder is an error, not an empty listing.
        let err = names_in(&d.path().join("status")).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::ENOTDIR));
    }

    #[test]
    #[allow(clippy::zombie_processes)] // the reap under test collects it
    fn reap_gives_up_on_a_child_that_stays_alive() {
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let t = Instant::now();
        assert_eq!(reap(child.id(), Duration::from_millis(200)), None);
        let waited = t.elapsed();
        assert!(
            waited >= Duration::from_millis(200) && waited < Duration::from_secs(5),
            "{waited:?}"
        );
        // The child is still there: a later reap, after it ends, collects it.
        child.kill().unwrap();
        let cpu = reap(child.id(), Duration::from_secs(10)).unwrap();
        assert!((0.0..1.0).contains(&cpu), "{cpu}");
    }

    #[test]
    #[allow(clippy::zombie_processes)] // the reap under test collects it
    fn reap_returns_the_cpu_of_a_child_that_has_exited() {
        let child = Command::new("true").spawn().unwrap();
        let cpu = reap(child.id(), Duration::from_secs(10)).unwrap();
        assert!((0.0..1.0).contains(&cpu), "{cpu}");
        // A process that is not the box's child counts nothing.
        assert_eq!(reap(1, Duration::from_millis(10)), Some(0.0));
    }
}
