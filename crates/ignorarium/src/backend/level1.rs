//! Level 1: each row runs in a bubblewrap sandbox. It gets new user, mount, PID, network,
//! IPC, UTS and cgroup namespaces, an empty read-only root with read-only binds of the
//! environment, the input folder and the payload, one writable output folder, and a
//! seccomp filter. See the bubblewrap manual: <https://github.com/containers/bubblewrap>.
use super::selftest::SelfTest;
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
/// Time to wait for the sandbox's PID 1 to become reapable once bubblewrap is gone. A
/// payload stuck in uninterruptible sleep outlasts a kill, and must not hold the box.
const REAP_LIMIT: Duration = Duration::from_secs(5);
const REAP_POLL: Duration = Duration::from_millis(5);
/// Where the box binary appears in the self-test sandbox.
const BOX_PATH: &str = "/box/ignorarium";

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
        }
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
        // `sys::inherit_only` needs this call in every spawn. Test it here, so a kernel
        // without it is named, not blamed on bubblewrap's "Invalid argument".
        if let Err(e) = (self.close_range)() {
            return Some(format!(
                "level 1 needs Linux 5.11 or later (close_range with CLOSE_RANGE_CLOEXEC): {e}"
            ));
        }
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
        let started = Started {
            held: Cell::new(Some((pipes.filter, pipes.status_w))),
            inner: watch,
        };
        let (exit, cpu) = supervise(cmd, job, start, &started);
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
        let done = |exit| Launched {
            exit,
            cpu,
            cpu_unknown: cpu_unknown.clone(),
            stdout: stdout.clone(),
        };
        // `Payload(TIMEOUT_CODE)` without an exit document is the wall timeout; with one,
        // bubblewrap passed on the payload's own 124, which the document holds too.
        if matches!(
            exit,
            Exit::Stopped | Exit::Infra(_) | Exit::Payload(TIMEOUT_CODE)
        ) {
            return done(exit);
        }
        let exit = match field(&status, "exit-code") {
            Some(0) => Exit::Ok,
            Some(c) => Exit::Payload(c),
            None => Exit::Infra(format!(
                "bubblewrap reported no payload exit: {}",
                stderr.trim()
            )),
        };
        done(exit)
    }

    /// The self-test sandbox: the host root, the box binary and the isolation of a row.
    /// `Ok` holds what the self-test found; `Err` is the row's exit when the test failed
    /// or could not run, with what it found when it printed a readable result.
    fn self_test(
        &self,
        job: &RowJob,
        start: Instant,
        watch: &dyn Watch,
    ) -> Result<SelfTest, (Exit, Option<SelfTest>)> {
        let mut mounts = root_mounts(Path::new("/"));
        mounts.extend(op("--ro-bind", &self.exe, BOX_PATH));
        let sandbox = Sandbox {
            mounts,
            run: os_args(["--", BOX_PATH, "dev", "selftest"]),
        };
        let probe_job = RowJob {
            wall_timeout: SMOKE_TIMEOUT.min(job.wall_timeout),
            ..RowJob::default()
        };
        let l = self.launch(&sandbox, &probe_job, start, watch, true);
        if l.exit == Exit::Stopped {
            return Err((Exit::Stopped, None));
        }
        let found = serde_json::from_str::<SelfTest>(l.stdout.trim()).ok();
        let why = match (&found, &l.exit) {
            (Some(t), Exit::Ok) if t.ok() => return Ok(*t),
            (Some(t), _) if !t.ok() => format!("{} was not blocked", t.failed().join(" and ")),
            _ => format!("it ended with {:?}: {}", l.exit, l.stdout.trim()),
        };
        Err((
            Exit::Infra(format!("sandbox self-test failed: {why}")),
            found,
        ))
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
        let sandbox = match row_args(job) {
            Ok(s) => s,
            Err(why) => return Ok(outcome(job, start, Exit::Infra(why), 0.0)),
        };
        let selftest = match self.self_test(job, start, watch) {
            Ok(t) => Some(t),
            Err((exit, found)) => {
                return Ok(RowOutcome {
                    selftest: found,
                    ..outcome(job, start, exit, 0.0)
                });
            }
        };
        let l = self.launch(&sandbox, job, start, watch, false);
        Ok(RowOutcome {
            selftest,
            cpu_unknown: l.cpu_unknown,
            ..outcome(job, start, l.exit, l.cpu)
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
/// reaped used; 0 when it is not the box's child. The sandbox's PID 1 dies with bubblewrap
/// (`--die-with-parent`), so the wait is short. `None` when it has not exited after
/// `limit`; the box, a subreaper, collects it later.
fn reap(pid: u32, limit: Duration) -> Option<f64> {
    let deadline = Instant::now() + limit;
    loop {
        match sys::has_exited(pid) {
            Err(_) => return Some(0.0),
            Ok(true) => {
                return Some(match sys::wait4(pid, true) {
                    Ok(Some(r)) => r.cpu_seconds,
                    _ => 0.0,
                });
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
}

impl Launched {
    fn infra(why: String) -> Launched {
        Launched {
            exit: Exit::Infra(why),
            cpu: 0.0,
            cpu_unknown: None,
            stdout: String::new(),
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
