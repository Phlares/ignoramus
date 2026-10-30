//! Level 1: rows in a bubblewrap sandbox. A test that launches one prints
//! `skip level1: <reason>` and returns where bubblewrap cannot run.
mod common;
use common::{
    PAYLOAD, REQUIRE_LEVEL1, in_child, is_child, lab, plain, register_study, rows_of, skip_level1,
    write_exec,
};
use ignorarium::backend::level0::Level0;
use ignorarium::backend::level1::{Level1, isolation_args, row_args};
use ignorarium::backend::{Backend, Exit, Probe, RowJob, Watch, output_hash, seccomp};
use ignorarium::machine;
use ignorarium::runner::{self, RunError, RunStatus, prepare, prepare_with};
use std::cell::Cell;
use std::ffi::{OsStr, OsString};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;

const LONG: Duration = Duration::from_secs(30);
const BOX: &str = env!("CARGO_BIN_EXE_ignorarium");

fn backend() -> Level1 {
    Level1::with(Level1::new().bwrap, BOX.into())
}

/// The backend, or `None` once the reason this host cannot run bubblewrap is printed.
/// Where the host can, the backend's own probe must agree.
fn ready() -> Option<Level1> {
    if skip_level1() {
        return None;
    }
    let b = backend();
    assert_eq!(b.probe(), Probe::Available);
    Some(b)
}

/// A job whose payload runs `script` with `/bin/sh`, with the arguments `--condition a`.
/// Its input folder holds `seen`; its output folder starts empty.
fn row(script: &str, timeout: Duration) -> (TempDir, RowJob) {
    let d = TempDir::new().unwrap();
    for f in ["in", "out", "payload"] {
        std::fs::create_dir(d.path().join(f)).unwrap();
    }
    std::fs::write(d.path().join("in/seen"), "input").unwrap();
    let payload = d.path().join("payload/run");
    write_exec(&payload, &format!("#!/bin/sh\n{script}\n"));
    let job = RowJob {
        argv: vec![
            payload.to_string_lossy().into(),
            "--condition".into(),
            "a".into(),
        ],
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        input_dir: d.path().join("in"),
        output_dir: d.path().join("out"),
        seed: [0; 32],
        wall_timeout: timeout,
        rootfs: None,
    };
    (d, job)
}

fn read(job: &RowJob, name: &str) -> String {
    std::fs::read_to_string(job.output_dir.join(name)).unwrap()
}

fn sorted_lines(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text.lines().map(str::to_string).collect();
    v.sort();
    v
}

/// The rootfs entries of the host root: (name, link target when a link).
fn host_entries() -> Vec<(&'static str, Option<PathBuf>)> {
    ["usr", "bin", "lib", "lib64", "etc"]
        .into_iter()
        .filter_map(|e| {
            let p = Path::new("/").join(e);
            match std::fs::read_link(&p) {
                Ok(t) => Some((e, Some(t))),
                Err(_) if p.is_dir() => Some((e, None)),
                Err(_) => None,
            }
        })
        .collect()
}

/// Every path under `dir`, relative, sorted.
fn tree(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            out.push(p.strip_prefix(base).unwrap().display().to_string());
            if p.is_dir() {
                walk(&p, base, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Live processes whose command line holds `marker`.
fn alive(marker: &str) -> Vec<u32> {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|e| {
            let pid: u32 = e.ok()?.file_name().to_str()?.parse().ok()?;
            let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            let state = stat
                .rsplit_once(')')?
                .1
                .split_whitespace()
                .next()?
                .to_string();
            let hit = cmd.windows(marker.len()).any(|w| w == marker.as_bytes());
            (hit && state != "Z").then_some(pid)
        })
        .collect()
}

/// True once no process holding `marker` is alive, polled for up to `limit`.
fn gone_within(marker: &str, limit: Duration) -> bool {
    let t = Instant::now();
    while !alive(marker).is_empty() {
        if t.elapsed() > limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

fn marker(name: &str) -> String {
    format!("ignorarium-test-{name}-{}", std::process::id())
}

#[test]
fn level1_runs_a_payload_and_hashes_its_output() {
    let Some(b) = ready() else { return };
    let (_d, job) = row(
        "printf 42 > /out/result; printf '%s|%s %s' \"$0\" \"$1\" \"$2\" > /out/argv",
        LONG,
    );
    let o = b.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Ok);
    assert_eq!(read(&job, "result"), "42");
    assert_eq!(read(&job, "argv"), "/payload/run|--condition a");
    assert_eq!(o.output_hash, output_hash(&job.output_dir).unwrap());
    assert!(o.wall_seconds > 0.0 && o.wall_seconds < 30.0);
    assert_eq!(b.level(), 1);
}

#[test]
fn level1_counts_the_payloads_cpu_time() {
    let Some(b) = ready() else { return };
    // About 0.4 s of CPU in the payload, two processes below bubblewrap.
    let (_d, job) = row("i=0; while [ $i -lt 400000 ]; do i=$((i+1)); done", LONG);
    let o = b.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Ok);
    assert!(
        o.cpu_seconds > 0.1 && o.cpu_seconds <= o.wall_seconds + 0.05,
        "cpu {} wall {}",
        o.cpu_seconds,
        o.wall_seconds
    );
}

#[test]
fn payload_sees_only_declared_paths() {
    let Some(b) = ready() else { return };
    let (_d, job) = row(
        "ls / > /out/root; ls /dev > /out/dev; \
         for e in usr bin lib lib64 etc; do [ -L /$e ] && echo \"$e $(readlink /$e)\"; done > /out/links; true",
        LONG,
    );
    assert_eq!(b.run_row(&job, &|| true).unwrap().exit, Exit::Ok);
    let host = host_entries();
    let mut want: Vec<String> = host.iter().map(|(e, _)| e.to_string()).collect();
    want.extend(["dev", "in", "out", "payload", "proc"].map(String::from));
    want.sort();
    assert_eq!(sorted_lines(&read(&job, "root")), want);
    assert_eq!(
        sorted_lines(&read(&job, "dev")),
        ["null", "urandom", "zero"]
    );
    let links: Vec<String> = host
        .iter()
        .filter_map(|(e, t)| Some(format!("{e} {}", t.as_ref()?.display())))
        .collect();
    assert_eq!(sorted_lines(&read(&job, "links")), {
        let mut l = links;
        l.sort();
        l
    });
}

#[test]
fn payload_sees_in_and_out() {
    let Some(b) = ready() else { return };
    let (_d, job) = row(
        "tr '\\0' '\\n' < /proc/$$/environ > /out/env; cat \"$IGNORARIUM_IN/seen\" > /out/seen; pwd > /out/pwd",
        LONG,
    );
    assert_eq!(b.run_row(&job, &|| true).unwrap().exit, Exit::Ok);
    assert_eq!(
        sorted_lines(&read(&job, "env")),
        [
            "IGNORARIUM_IN=/in",
            "IGNORARIUM_OUT=/out",
            "PATH=/usr/bin:/bin",
            "PWD=/out"
        ]
    );
    assert_eq!(read(&job, "seen"), "input");
    assert_eq!(read(&job, "pwd"), "/out\n");
    // A study with no input folder runs, and finds no /in, as it would at level 0.
    let (d, mut job) = row(
        "[ -e /in ] && echo present > /out/in || echo absent > /out/in",
        LONG,
    );
    job.input_dir = d.path().join("no-input");
    assert_eq!(b.run_row(&job, &|| true).unwrap().exit, Exit::Ok);
    assert_eq!(read(&job, "in"), "absent\n");
}

#[test]
fn payload_cannot_write_outside_out() {
    let Some(b) = ready() else { return };
    let (d, job) = row(
        "for t in /escape /in/escape /payload/escape /usr/escape /etc/escape /dev/escape /tmp/escape; do \
           if touch \"$t\" 2>/dev/null; then echo \"$t written\"; else echo \"$t denied\"; fi; \
         done > /out/tries; \
         for f in /in/seen /payload/run; do \
           if echo x >> \"$f\" 2>/dev/null; then echo \"$f written\"; else echo \"$f denied\"; fi; \
         done >> /out/tries; \
         touch /out/inside",
        LONG,
    );
    let before: Vec<String> = tree(d.path())
        .into_iter()
        .filter(|p| !p.starts_with("out"))
        .collect();
    let seen_before = std::fs::read(d.path().join("in/seen")).unwrap();
    let payload_before = std::fs::read(d.path().join("payload/run")).unwrap();
    assert_eq!(b.run_row(&job, &|| true).unwrap().exit, Exit::Ok);
    let tries = read(&job, "tries");
    assert_eq!(
        tries.lines().collect::<Vec<_>>(),
        [
            "/escape denied",
            "/in/escape denied",
            "/payload/escape denied",
            "/usr/escape denied",
            "/etc/escape denied",
            "/dev/escape denied",
            "/tmp/escape denied",
            "/in/seen denied",
            "/payload/run denied",
        ]
    );
    let after: Vec<String> = tree(d.path())
        .into_iter()
        .filter(|p| !p.starts_with("out"))
        .collect();
    assert_eq!(after, before);
    assert_eq!(
        std::fs::read(d.path().join("in/seen")).unwrap(),
        seen_before
    );
    assert_eq!(
        std::fs::read(d.path().join("payload/run")).unwrap(),
        payload_before
    );
    assert_eq!(tree(&job.output_dir), ["inside", "tries"]);
}

#[test]
fn network_is_off_inside() {
    let Some(b) = ready() else { return };
    // TEST-NET-1 (RFC 5737): never routed, so only a missing route fails at once.
    let addr = Ipv4Addr::new(192, 0, 2, 1);
    let (_d, job) = row(
        &format!(
            "cat /proc/net/dev > /out/netdev; \
             if bash -c 'exec 3<>/dev/tcp/{addr}/9' 2>/dev/null; then echo open; else echo closed; fi > /out/connect"
        ),
        LONG,
    );
    let o = b.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Ok);
    let names: Vec<String> = read(&job, "netdev")
        .lines()
        .skip(2)
        .filter_map(|l| Some(l.split_once(':')?.0.trim().to_string()))
        .collect();
    assert_eq!(names, ["lo"]);
    assert_eq!(read(&job, "connect"), "closed\n");
    assert!(o.wall_seconds < 5.0, "{}", o.wall_seconds);
}

#[test]
fn no_new_privileges_and_no_capabilities() {
    let Some(b) = ready() else { return };
    let (_d, job) = row(
        "grep -E '^(NoNewPrivs|CapEff):' /proc/self/status > /out/status",
        LONG,
    );
    assert_eq!(b.run_row(&job, &|| true).unwrap().exit, Exit::Ok);
    assert_eq!(
        read(&job, "status").lines().collect::<Vec<_>>(),
        ["CapEff:\t0000000000000000", "NoNewPrivs:\t1"]
    );
}

#[test]
fn payload_inherits_no_file_descriptors_from_the_box() {
    let Some(b) = ready() else { return };
    let (d, job) = row("ls /proc/self/fd > /out/fds", LONG);
    // A writable host file the box holds without close-on-exec, as a parent's
    // `exec 3>file` leaves one.
    let host = std::fs::File::create(d.path().join("host-file")).unwrap();
    rustix::io::fcntl_setfd(&host, rustix::io::FdFlags::empty()).unwrap();
    let o = b.run_row(&job, &|| true).unwrap();
    drop(host);
    assert_eq!(o.exit, Exit::Ok);
    // stdin, stdout, stderr, and the folder `ls` reads.
    assert_eq!(sorted_lines(&read(&job, "fds")), ["0", "1", "2", "3"]);
}

/// The namespaces level 1 unshares. bubblewrap has no time namespace option.
const UNSHARED: [&str; 7] = ["cgroup", "ipc", "mnt", "net", "pid", "user", "uts"];

#[test]
fn payload_runs_in_its_own_namespaces_and_session() {
    let Some(b) = ready() else { return };
    let (_d, job) = row(
        &format!(
            "for n in {}; do echo \"$n $(readlink /proc/self/ns/$n)\"; done > /out/ns; \
             if unshare -U true 2>/dev/null; then echo allowed; else echo refused; fi > /out/userns; \
             echo \"$$ $(cut -d' ' -f6 /proc/$$/stat)\" > /out/session",
            UNSHARED.join(" ")
        ),
        LONG,
    );
    assert_eq!(b.run_row(&job, &|| true).unwrap().exit, Exit::Ok);
    let inside = read(&job, "ns");
    for (line, n) in inside.lines().zip(UNSHARED) {
        let host = std::fs::read_link(format!("/proc/self/ns/{n}")).unwrap();
        assert_eq!(line.split_once(' ').unwrap().0, n);
        assert_ne!(
            line.split_once(' ').unwrap().1,
            host.to_str().unwrap(),
            "{n}"
        );
    }
    assert_eq!(inside.lines().count(), UNSHARED.len());
    assert_eq!(read(&job, "userns"), "refused\n");
    // The sandbox's PID 1 leads a session of its own, and the payload (PID 2) is in it.
    // Without a new session the payload's session lies outside the namespace: 0.
    assert_eq!(read(&job, "session"), "2 1\n");
}

#[test]
fn isolation_args_are_the_constraint_list() {
    let want = [
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
        "10",
        "--json-status-fd",
        "11",
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
    ];
    assert_eq!(isolation_args(10, 11), want.map(OsString::from));
}

fn os(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

#[test]
fn row_args_bind_folders_read_only_and_recreate_links() {
    let d = TempDir::new().unwrap();
    let root = d.path().join("root");
    std::fs::create_dir_all(root.join("usr/bin")).unwrap();
    std::os::unix::fs::symlink("usr/bin", root.join("bin")).unwrap();
    // A plain file is neither a folder nor a link: it adds nothing.
    std::fs::write(root.join("etc"), "").unwrap();
    let (_r, mut job) = row("true", LONG);
    job.rootfs = Some(root.clone());
    job.env = vec![("A".into(), "1".into())];
    let s = row_args(&job).unwrap();
    let text = |p: &Path| p.to_str().unwrap().to_string();
    let payload_dir = Path::new(&job.argv[0]).parent().unwrap();
    assert_eq!(
        s.mounts,
        os(&[
            "--ro-bind",
            &text(&root.join("usr")),
            "/usr",
            "--symlink",
            "usr/bin",
            "/bin",
            "--ro-bind-try",
            &text(&job.input_dir),
            "/in",
            "--bind",
            &text(&job.output_dir),
            "/out",
            "--ro-bind",
            &text(payload_dir),
            "/payload",
        ])
    );
    assert_eq!(
        s.run,
        os(&[
            "--setenv",
            "A",
            "1",
            "--setenv",
            "IGNORARIUM_IN",
            "/in",
            "--setenv",
            "IGNORARIUM_OUT",
            "/out",
            "--chdir",
            "/out",
            "--",
            "/payload/run",
            "--condition",
            "a",
        ])
    );
    // An environment root that is not a folder is refused.
    for bad in [root.join("etc"), d.path().join("missing")] {
        job.rootfs = Some(bad.clone());
        assert_eq!(
            row_args(&job).err(),
            Some(format!(
                "environment root {} is not a folder",
                bad.display()
            ))
        );
    }
}

#[test]
fn a_required_level1_turns_a_skip_into_a_failure() {
    const NAME: &str = "a_required_level1_turns_a_skip_into_a_failure";
    if is_child(NAME) {
        assert!(skip_level1());
        return;
    }
    // No bubblewrap on PATH, so level 1 cannot run in the child.
    let d = TempDir::new().unwrap();
    let path = d.path().as_os_str();
    assert!(in_child(
        NAME,
        d.path(),
        &[("PATH", path), (REQUIRE_LEVEL1, OsStr::new("0"))]
    ));
    assert!(!in_child(
        NAME,
        d.path(),
        &[("PATH", path), (REQUIRE_LEVEL1, OsStr::new("1"))]
    ));
}

/// Notes once a process holding `marker` runs.
struct Spy {
    marker: String,
    seen: Cell<bool>,
}

impl Watch for Spy {
    fn tick(&self) -> bool {
        if !self.seen.get() && !alive(&self.marker).is_empty() {
            self.seen.set(true);
        }
        true
    }
}

#[test]
fn a_detached_grandchild_dies_with_the_row() {
    let Some(b) = ready() else { return };
    let m = marker("detached");
    let (_d, job) = row(
        &format!("setsid sh -c 'sleep 60; : {m}' & sleep 60"),
        Duration::from_secs(2),
    );
    let spy = Spy {
        marker: m.clone(),
        seen: Cell::new(false),
    };
    let o = b.run_row(&job, &spy).unwrap();
    assert_eq!(o.exit, Exit::Payload(124));
    assert!(spy.seen.get(), "the detached grandchild never ran");
    assert!(gone_within(&m, Duration::from_secs(1)), "{:?}", alive(&m));
}

/// Kills the sandbox leader with SIGKILL once a process holding `marker` runs, then
/// records whether that process died within a second. The box's own group kill comes
/// only after this tick returns, so the death is the sandbox's own doing.
struct Killer {
    marker: String,
    leader: Cell<Option<u32>>,
    died: Cell<Option<bool>>,
}

impl Watch for Killer {
    fn tick(&self) -> bool {
        if let (Some(pid), None) = (self.leader.get(), self.died.get())
            && !alive(&self.marker).is_empty()
        {
            let pid = rustix::process::Pid::from_raw(pid as i32).unwrap();
            rustix::process::kill_process(pid, rustix::process::Signal::KILL).unwrap();
            self.died
                .set(Some(gone_within(&self.marker, Duration::from_secs(1))));
        }
        true
    }
    fn started(&self, pid: u32) {
        self.leader.set(Some(pid));
    }
}

#[test]
fn killing_bwrap_ends_the_sandbox() {
    let Some(b) = ready() else { return };
    let m = marker("killed");
    let (_d, job) = row(&format!("sh -c 'sleep 60; : {m}'"), LONG);
    let k = Killer {
        marker: m.clone(),
        leader: Cell::new(None),
        died: Cell::new(None),
    };
    let o = b.run_row(&job, &k).unwrap();
    assert_eq!(k.died.get(), Some(true), "{:?}", alive(&m));
    // bubblewrap died before it could report the payload's exit.
    assert!(
        matches!(&o.exit, Exit::Infra(m) if m.starts_with("bubblewrap reported no payload exit")),
        "{:?}",
        o.exit
    );
}

#[test]
fn a_bad_bind_is_an_infra_fault_with_bwraps_reason() {
    let Some(b) = ready() else { return };
    // A missing environment root is refused before bubblewrap runs.
    let (d, mut job) = row("printf ran > /out/ran", LONG);
    job.rootfs = Some(d.path().join("no-root"));
    let o = b.run_row(&job, &|| true).unwrap();
    assert_eq!(
        o.exit,
        Exit::Infra(format!(
            "environment root {} is not a folder",
            d.path().join("no-root").display()
        ))
    );
    assert!(!job.output_dir.join("ran").exists());
    // A missing output folder cannot be bound.
    let (d, mut job) = row("printf ran > /out/ran", LONG);
    job.output_dir = d.path().join("no-output");
    let o = b.run_row(&job, &|| true).unwrap();
    assert_eq!(
        o.exit,
        Exit::Infra(format!(
            "bubblewrap reported no payload exit: \
             bwrap: Can't find source path {}: No such file or directory",
            job.output_dir.display()
        ))
    );
    assert!(!job.output_dir.exists());
    job.argv.clear();
    assert_eq!(
        b.run_row(&job, &|| true).unwrap().exit,
        Exit::Infra("empty argv".into())
    );
}

#[test]
fn payload_exit_codes_pass_through() {
    let Some(b) = ready() else { return };
    let exit_of = |script: &str, timeout: Duration, watch: &dyn Watch| {
        let (_d, job) = row(script, timeout);
        let o = b.run_row(&job, watch).unwrap();
        (o.exit, o.wall_seconds)
    };
    assert_eq!(exit_of("exit 3", LONG, &|| true).0, Exit::Payload(3));
    assert_eq!(exit_of("kill -9 $$", LONG, &|| true).0, Exit::Payload(137));
    let (exit, wall) = exit_of("sleep 30", Duration::from_millis(300), &|| true);
    assert_eq!(exit, Exit::Payload(124));
    assert!((0.3..5.0).contains(&wall), "{wall}");
    // A payload's own 124 before the deadline is its result too.
    assert_eq!(exit_of("exit 124", LONG, &|| true).0, Exit::Payload(124));
    assert_eq!(exit_of("sleep 30", LONG, &|| false).0, Exit::Stopped);
}

/// A stand-in bubblewrap that prints `bubblewrap <version>` and refuses to sandbox.
fn fake(dir: &Path, version: &str) -> Level1 {
    fake_running(dir, version, "echo 'fake: no sandbox here' >&2")
}

/// A stand-in bubblewrap that prints `bubblewrap <version>` and, asked for a sandbox,
/// runs `script` and exits 1.
fn fake_running(dir: &Path, version: &str, script: &str) -> Level1 {
    let path = dir.join(format!("bwrap-{version}-{}", script.len()));
    write_exec(
        &path,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'bubblewrap {version}'; exit 0; fi\n\
             {script}\nexit 1\n"
        ),
    );
    Level1::with(path, BOX.into())
}

#[test]
fn a_setup_fault_keeps_the_last_4_kib_of_stderr() {
    let d = TempDir::new().unwrap();
    // 10 KiB of `x`, then a marker, on stderr.
    let b = fake_running(
        d.path(),
        "0.9.0",
        "head -c 10240 /dev/zero | tr '\\0' x >&2; printf 'END-OF-STDERR' >&2",
    );
    let tail = format!("{}END-OF-STDERR", "x".repeat(4096 - "END-OF-STDERR".len()));
    assert_eq!(
        why(&b),
        format!("bubblewrap reported no payload exit: {tail}")
    );
}

/// A stand-in bubblewrap. The self-test sandbox, which names `/box/ignorarium`, prints a
/// passing result. A row sandbox (one that names `/payload/`) leaves a process behind,
/// reported as the sandbox's PID 1, in a session of its own so the group kill misses it,
/// holding none of the pipes. The fake waits 0.3 s so the process has left the group before
/// the group kill. Any other sandbox, the probe's, just succeeds. With `report_exit` the row
/// sandbox also reports an exit code; without it the row is an infrastructure fault. With
/// `once`, only the first row sandbox leaves the process behind.
fn stuck_bwrap(dir: &Path, report_exit: bool, once: bool) -> Level1 {
    let path = dir.join(format!("bwrap-stuck-{report_exit}-{once}"));
    let marker = dir.join(format!("stuck-seen-{report_exit}-{once}"));
    let script = r#"#!/bin/bash
if [ "$1" = --version ]; then echo 'bubblewrap 0.9.0'; exit 0; fi
prev=; for a in "$@"; do [ "$prev" = --json-status-fd ] && fd=$a; prev=$a; done
case "$*" in *' /box/ignorarium '*)
  echo '{"connect_blocked":true,"vsock_blocked":true,"io_uring_blocked":true}'
  echo '{"exit-code": 0}' >&$fd
  exit 0;; esac
case "$*" in *' /payload/'*) ;; *) echo '{"exit-code": 0}' >&$fd; exit 0;; esac
if [ -n "@@ONCE@@" ] && [ -e "@@MARKER@@" ]; then echo '{"exit-code": 0}' >&$fd; exit 0; fi
: > "@@MARKER@@"
pid=$( eval "exec $fd>&-"; setsid sleep 20 >/dev/null 2>&1 & echo $! )
sleep 0.3
echo "{\"child-pid\": $pid}" >&$fd
if [ -n "@@REPORT@@" ]; then echo '{"exit-code": 0}' >&$fd; fi
exit 0
"#;
    let script = script
        .replace("@@MARKER@@", &marker.to_string_lossy())
        .replace("@@ONCE@@", if once { "y" } else { "" })
        .replace("@@REPORT@@", if report_exit { "y" } else { "" });
    write_exec(&path, &script);
    Level1::with(path, BOX.into())
}

fn progress_of(lab: &common::Lab, id: &str) -> runner::Progress {
    let key = ignorarium::keys::load(&lab.home).unwrap().unwrap();
    let log = lab.home.study_dir(id).join("records.jsonl");
    let all = ignorarium::records::read_all(&log, &key.verifying_key()).unwrap();
    runner::progress(&lab.home, id, &all).unwrap()
}

/// The study's per-row CPU estimate in seconds.
fn per_row_cpu(lab: &common::Lab, id: &str) -> f64 {
    let spec = progress_of(lab, id).spec;
    let m = machine::load(&lab.home.root).unwrap();
    let e = ignorarium::estimate::estimate(&spec, &m, ignorarium::estimate::units(&spec));
    e.cpu_hours * 3600.0 / e.rows.max(1) as f64
}

#[test]
fn a_stuck_sandbox_pid_1_marks_the_cpu_unknown_and_the_row_goes_on() {
    let d = TempDir::new().unwrap();
    let b = stuck_bwrap(d.path(), true, false);
    let (_w, job) = row("true", LONG);
    let t = Instant::now();
    let o = b.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Ok);
    assert_eq!(
        o.cpu_unknown.as_deref(),
        Some("the sandbox's PID 1 did not exit in time; its CPU time is unknown")
    );
    // The wait ends at the limit: it neither returns at once nor hangs.
    let waited = t.elapsed();
    assert!(
        waited >= Duration::from_secs(5) && waited < Duration::from_secs(15),
        "{waited:?}"
    );
}

const UNKNOWN: &str = "the sandbox's PID 1 did not exit in time; its CPU time is unknown";

/// A stand-in bubblewrap whose self-test sandbox prints `result`, reports `code` and leaves
/// a process behind as its PID 1, as `stuck_bwrap` does for a row. Other sandboxes succeed.
fn stuck_selftest_bwrap(dir: &Path, result: &str, code: i32) -> Level1 {
    let path = dir.join("bwrap-stuck-selftest");
    let script = format!(
        r#"#!/bin/bash
if [ "$1" = --version ]; then echo 'bubblewrap 0.9.0'; exit 0; fi
prev=; for a in "$@"; do [ "$prev" = --json-status-fd ] && fd=$a; prev=$a; done
case "$*" in *' /box/ignorarium '*) ;; *) echo '{{"exit-code": 0}}' >&$fd; exit 0;; esac
pid=$( eval "exec $fd>&-"; setsid sleep 20 >/dev/null 2>&1 & echo $! )
sleep 0.3
echo '{result}'
echo "{{\"child-pid\": $pid}}" >&$fd
echo '{{"exit-code": {code}}}' >&$fd
exit 0
"#
    );
    write_exec(&path, &script);
    Level1::with(path, BOX.into())
}

#[test]
fn a_failed_self_test_with_a_stuck_pid_1_marks_the_rows_cpu_unknown() {
    let d = TempDir::new().unwrap();
    let failed = r#"{"connect_blocked":false,"vsock_blocked":true,"io_uring_blocked":true}"#;
    let b = stuck_selftest_bwrap(d.path(), failed, 102);
    let (_w, job) = row("true", LONG);
    let o = b.run_row(&job, &|| true).unwrap();
    assert_eq!(
        o.exit,
        Exit::Infra("sandbox self-test failed: connect was not blocked".into())
    );
    assert_eq!(o.cpu_unknown.as_deref(), Some(UNKNOWN));
}

#[test]
fn a_passed_self_test_with_a_stuck_pid_1_marks_the_rows_cpu_unknown() {
    let d = TempDir::new().unwrap();
    let passed = r#"{"connect_blocked":true,"vsock_blocked":true,"io_uring_blocked":true}"#;
    let b = stuck_selftest_bwrap(d.path(), passed, 0);
    let (_w, job) = row("true", LONG);
    let o = b.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Ok);
    assert_eq!(o.cpu_unknown.as_deref(), Some(UNKNOWN));
}

#[test]
fn an_unknown_cpu_row_is_charged_the_estimate_warned_about_and_pauses_the_run() {
    let lab = lab();
    let dir = plain(&lab, "unk");
    let d = TempDir::new().unwrap();
    let plan = prepare_with(
        &lab.home,
        "unk",
        vec![Box::new(stuck_bwrap(d.path(), true, false))],
    )
    .unwrap();
    let status = runner::run(&plan).unwrap();
    let RunStatus::Paused(why) = status else {
        panic!("{status:?}")
    };
    assert_eq!(
        why,
        "row 0 has an unknown CPU count; it is charged the row estimate"
    );
    let rows = rows_of(&lab, "unk");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].body["cpu_unknown"], UNKNOWN);
    // The row is charged the estimate, not the near-zero it measured.
    let est = per_row_cpu(&lab, "unk");
    assert!(est > 1.0, "{est}");
    assert_eq!(rows[0].body["cpu_seconds"].as_f64().unwrap(), est);
    // The warn event names the row.
    let events = common::events_of(&dir);
    let warns = common::named(&events, "row.cpu_unknown");
    assert_eq!(warns.len(), 1);
    assert_eq!(warns[0]["level"], "warn");
    assert_eq!(warns[0]["attrs"]["row"], 0);
    assert_eq!(warns[0]["attrs"]["reason"], UNKNOWN);
    assert_eq!(
        warns[0]["attrs"]["charged_cpu_seconds"].as_f64().unwrap(),
        est
    );
    // Status counts it, and the cost carries the charge.
    let p = progress_of(&lab, "unk");
    assert_eq!(p.unknown_cpu, 1);
    assert_eq!(p.cost.cpu_seconds, est);
}

#[test]
fn an_unknown_cpu_attempt_that_is_retried_leaves_a_spend_record_and_still_pauses() {
    let lab = lab();
    plain(&lab, "unk2");
    let d = TempDir::new().unwrap();
    // The first attempt is a fault with a stuck PID 1; the retry runs clean.
    let plan = prepare_with(
        &lab.home,
        "unk2",
        vec![Box::new(stuck_bwrap(d.path(), false, true))],
    )
    .unwrap();
    let status = runner::run(&plan).unwrap();
    let RunStatus::Paused(why) = status else {
        panic!("{status:?}")
    };
    assert_eq!(
        why,
        "row 0 has an unknown CPU count; it is charged the row estimate"
    );
    let spends = common::kind_of(&lab, "unk2", "spend");
    assert_eq!(spends.len(), 1);
    assert_eq!(spends[0].body["cpu_unknown"], UNKNOWN);
    assert_eq!(
        spends[0].body["cpu_seconds"].as_f64().unwrap(),
        per_row_cpu(&lab, "unk2")
    );
    let rows = rows_of(&lab, "unk2");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].body.get("cpu_unknown").is_none());
    assert_eq!(rows[0].body["attempts"], 2);
}

fn why(b: &Level1) -> String {
    match b.probe() {
        Probe::Unavailable(why) => why,
        Probe::Available => panic!("{} probed available", b.bwrap.display()),
    }
}

#[test]
fn probe_reports_why_level1_is_unavailable() {
    let missing = Level1::with("/nonexistent/bwrap".into(), BOX.into());
    let w = why(&missing);
    assert!(w.starts_with("cannot run /nonexistent/bwrap: "), "{w}");
    assert_eq!(missing.launcher(), "bubblewrap unknown");
    let d = TempDir::new().unwrap();
    for old in ["0.7.0", "0.7.99", "0.0.1"] {
        assert_eq!(
            why(&fake(d.path(), old)),
            format!("bubblewrap {old} is older than 0.8, which level 1 needs for --disable-userns")
        );
    }
    let unreadable = fake(d.path(), "x.y");
    assert_eq!(
        why(&unreadable),
        format!(
            "{} --version printed \"bubblewrap x.y\", not a version",
            unreadable.bwrap.display()
        )
    );
    // At or past the floor, the test sandbox runs and its failure is the reason.
    for new in ["0.8.0", "0.10.0", "1.0.0"] {
        let b = fake(d.path(), new);
        assert_eq!(
            why(&b),
            "bubblewrap reported no payload exit: fake: no sandbox here"
        );
        assert_eq!(b.launcher(), format!("bubblewrap {new}"));
    }
    // A machine that cannot do level 1 probes level 0.
    let backends: Vec<Box<dyn Backend>> = vec![Box::new(Level0), Box::new(missing)];
    assert_eq!(
        machine::measure(d.path(), &machine::probe_backends(&backends))
            .unwrap()
            .max_level,
        0
    );
}

#[test]
fn launcher_names_the_bubblewrap_version() {
    let Some(b) = ready() else { return };
    let out = std::process::Command::new(&b.bwrap)
        .arg("--version")
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(b.launcher(), text.trim());
    assert!(
        b.launcher().starts_with("bubblewrap 0."),
        "{}",
        b.launcher()
    );
}

fn need_isolation(t: String) -> String {
    t.replace("min_isolation = 0", "min_isolation = 1")
}

#[test]
fn min_isolation_1_is_refused_without_level1() {
    let lab = lab();
    register_study(&lab, "need1", need_isolation, &[], PAYLOAD);
    let err = prepare_with(&lab.home, "need1", vec![Box::new(Level0)])
        .err()
        .unwrap();
    assert!(
        matches!(&err, RunError::NoBackend { need: 1, have } if *have == [0]),
        "{err}"
    );
    let err = prepare(&lab.home, "need1", 0).err().unwrap();
    assert!(
        matches!(&err, RunError::NoBackend { need: 1, have } if *have == [0]),
        "{err}"
    );
}

#[test]
fn prepare_prefers_the_highest_backend() {
    let Some(b) = ready() else { return };
    let lab = lab();
    let dir = plain(&lab, "hi");
    let launcher = b.launcher();
    let both: Vec<Box<dyn Backend>> = vec![Box::new(b), Box::new(Level0)];
    let plan = prepare_with(&lab.home, "hi", both).unwrap();
    assert_eq!(plan.backend.level(), 1);
    assert_eq!(prepare(&lab.home, "hi", 1).unwrap().backend.level(), 1);
    assert_eq!(prepare(&lab.home, "hi", 0).unwrap().backend.level(), 0);
    // The whole study runs at level 1, and every record says so.
    assert_eq!(runner::run(&plan).unwrap(), RunStatus::Done);
    let rows = rows_of(&lab, "hi");
    assert_eq!(rows.len(), 6);
    for r in &rows {
        assert_eq!(r.body["exit"], "ok");
        assert_eq!(r.body["isolation"], 1);
        assert_eq!(r.body["launcher"], launcher.as_str());
        let out = dir.join(format!("run/rows/{}/out", r.row.unwrap()));
        let want = format!(
            "{} {} {}",
            r.body["condition"].as_str().unwrap(),
            r.body["unit"].as_str().unwrap(),
            r.body["repeat"]
        );
        assert_eq!(std::fs::read_to_string(out.join("result")).unwrap(), want);
        assert_eq!(r.body["output_hash"], output_hash(&out).unwrap().as_str());
    }
}

#[test]
fn seccomp_program_is_a_whole_number_of_filters() {
    let p = seccomp::program();
    assert!(!p.is_empty());
    assert_eq!(p.len() % 8, 0);
    // Each instruction: code u16, jt u8, jf u8, k u32, native endian.
    let ins: Vec<(u16, u32)> = p
        .chunks(8)
        .map(|c| {
            (
                u16::from_ne_bytes([c[0], c[1]]),
                u32::from_ne_bytes([c[4], c[5], c[6], c[7]]),
            )
        })
        .collect();
    const JEQ: u16 = 0x15;
    const RET: u16 = 0x06;
    let mut compared: Vec<u32> = ins.iter().filter(|i| i.0 == JEQ).map(|i| i.1).collect();
    compared.sort();
    // x86_64 also takes each call through the x32 ABI: the same number with bit 30 set.
    #[cfg(target_arch = "x86_64")]
    let (arch, abis) = (0xC000_003E, [0, 0x4000_0000].as_slice());
    #[cfg(target_arch = "aarch64")]
    let (arch, abis) = (0xC000_00B7, [0].as_slice());
    let mut want = vec![arch];
    for abi in abis {
        for nr in [
            libc::SYS_io_uring_setup,
            libc::SYS_io_uring_enter,
            libc::SYS_io_uring_register,
            libc::SYS_socket,
        ] {
            want.push(nr as u32 | abi);
        }
        // Each `socket` entry compares its first argument with AF_VSOCK.
        want.push(libc::AF_VSOCK as u32);
    }
    want.sort();
    assert_eq!(compared, want);
    let mut returns: Vec<u32> = ins.iter().filter(|i| i.0 == RET).map(|i| i.1).collect();
    returns.sort();
    returns.dedup();
    const KILL_PROCESS: u32 = 0x8000_0000;
    const ERRNO_EPERM: u32 = 0x0005_0000 | libc::EPERM as u32;
    const ALLOW: u32 = 0x7fff_0000;
    assert_eq!(returns, [ERRNO_EPERM, ALLOW, KILL_PROCESS]);
}
