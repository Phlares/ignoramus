//! Level 2: each row in a fresh Cloud Hypervisor virtual machine. A test that boots one starts
//! with `ready()`, which prints `skip level2: <reason>` and returns `None` where this host
//! cannot run level 2.
mod common;
use common::{in_child, is_child, write_exec};
use ignorarium::backend::level0::Level0;
use ignorarium::backend::level1::Level1;
use ignorarium::backend::level2::{
    GUEST_MIB, Level2, guest_job, outcome_of, rootfs_disk, selftest_of,
};
use ignorarium::backend::selftest::SelfTest;
use ignorarium::backend::{
    Backend, Exit, Probe, RowJob, RowOutcome, TIMEOUT_CODE, Watch, output_hash, rootfs,
};
use ignorarium::runner::{self, RunError, RunStatus};
use ignorarium::store::Store;
use ignorarium_guest::host::{Timings, VmError, VmOutcome};
use ignorarium_guest::image::{KERNEL, VMM, sha256_file};
use ignorarium_guest::pack::Link;
use ignorarium_guest::proto::{self, End, Report};
use ignorarium_guest::{pack, tree};
use rustix::process::{Pid, getpgid, test_kill_process_group};
use serde_json::json;
use std::cell::{Cell, RefCell};
use std::ffi::OsStr;
use std::fs::{self, File, Permissions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Set to `1` to make every level 2 skip a failure.
const REQUIRE_LEVEL2: &str = "IGNORARIUM_REQUIRE_LEVEL2";
/// A `docker save` tar of the environment image the gated rows run in.
const TEST_ENVIRONMENT: &str = "IGNORARIUM_TEST_ENVIRONMENT";
const VARS: [&str; 3] = [
    "IGNORARIUM_VM_VMM",
    "IGNORARIUM_VM_KERNEL",
    "IGNORARIUM_VM_AGENT",
];
const LONG: Duration = Duration::from_secs(60);

struct Ready {
    level2: Level2,
    /// The environment root of the test image.
    root: PathBuf,
    /// The `docker save` tar the root came from.
    image: PathBuf,
}

/// The backend and the test environment, or `None` once the reason this host cannot run
/// level 2 is printed. With `IGNORARIUM_REQUIRE_LEVEL2=1` that reason fails the test.
fn ready() -> Option<Ready> {
    let level2 = Level2::new();
    let image = std::env::var_os(TEST_ENVIRONMENT).map(PathBuf::from);
    let why = match (level2.probe(), image) {
        (Probe::Unavailable(why), _) => why,
        (Probe::Available, None) => format!("{TEST_ENVIRONMENT} is not set"),
        (Probe::Available, Some(image)) => {
            static ROOT: OnceLock<PathBuf> = OnceLock::new();
            let root = ROOT.get_or_init(|| environment(&image)).clone();
            return Some(Ready {
                level2,
                root,
                image,
            });
        }
    };
    let required = std::env::var_os(REQUIRE_LEVEL2).is_some_and(|v| v == "1");
    assert!(!required, "level 2 is required here, but: {why}");
    println!("skip level2: {why}");
    None
}

/// The root of `image`, extracted into a store kept between runs, with its disk built, so
/// both happen once per image.
fn environment(image: &Path) -> PathBuf {
    let store = Store::open(&std::env::temp_dir().join("ignorarium-test-env")).unwrap();
    let digest = store.put_reader(&mut File::open(image).unwrap()).unwrap();
    let root = rootfs::from_store(&store, &digest).unwrap();
    rootfs_disk(&root).unwrap().unwrap();
    root
}

/// A job whose payload runs `script` with `/bin/sh`, with the arguments `--condition a`, in
/// the test environment. Its input folder holds `seen`; its output folder starts empty.
fn row(r: &Ready, script: &str, timeout: Duration) -> (TempDir, RowJob) {
    let d = TempDir::new().unwrap();
    for f in ["in", "out", "payload"] {
        fs::create_dir(d.path().join(f)).unwrap();
    }
    // The payload runs as an unprivileged guest user, so its trees must be open to others.
    for f in ["in", "payload"] {
        fs::set_permissions(d.path().join(f), Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(d.path().join("in/seen"), "input").unwrap();
    let payload = d.path().join("payload/run");
    write_exec(&payload, &format!("#!/bin/sh\n{script}\n"));
    let job = RowJob {
        argv: vec![
            payload.to_string_lossy().into(),
            "--condition".into(),
            "a".into(),
        ],
        env: vec![("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into())],
        input_dir: d.path().join("in"),
        output_dir: d.path().join("out"),
        seed: [0; 32],
        wall_timeout: timeout,
        rootfs: Some(r.root.clone()),
        ..RowJob::default()
    };
    (d, job)
}

fn read(job: &RowJob, name: &str) -> String {
    fs::read_to_string(job.output_dir.join(name)).unwrap()
}

/// A 64-bit little-endian ELF header with one loadable segment and no interpreter.
fn static_elf() -> Vec<u8> {
    let mut b = vec![0u8; 64 + 56];
    b[..6].copy_from_slice(b"\x7fELF\x02\x01");
    b[0x20..0x28].copy_from_slice(&64u64.to_le_bytes());
    b[0x36..0x38].copy_from_slice(&56u16.to_le_bytes());
    b[0x38..0x3a].copy_from_slice(&1u16.to_le_bytes());
    b[64..68].copy_from_slice(&1u32.to_le_bytes());
    b
}

fn with_kvm(kvm: &Path, vmm: &Path, kernel: &Path, agent: &Path) -> Level2 {
    let mut b = Level2::with(vmm.into(), kernel.into(), agent.into());
    b.kvm = kvm.into();
    b
}

/// The probe's faults, without the ones for `mkfs.ext4` and `debugfs`, which depend on the
/// host.
fn faults(b: &Level2) -> Vec<String> {
    let Probe::Unavailable(why) = b.probe() else {
        panic!("probed available")
    };
    let by_host = |f: &&str| {
        let tools = ["mkfs.ext4", "debugfs"];
        tools
            .iter()
            .any(|t| f.starts_with(&format!("{t} cannot run: ")))
    };
    why.split("; ")
        .filter(|f| !by_host(f))
        .map(str::to_string)
        .collect()
}

#[test]
fn probe_lists_every_fault() {
    const NAME: &str = "probe_lists_every_fault";
    let d = TempDir::new().unwrap();
    if !is_child(NAME) {
        // No `mkfs.ext4` on the child's PATH, so its fault reads the same on every host.
        assert!(in_child(NAME, d.path(), &[("PATH", d.path().as_os_str())]));
        return;
    }
    let kvm = d.path().join("kvm");
    let b = with_kvm(&kvm, Path::new(""), Path::new(""), Path::new(""));
    let want = format!(
        "{} is missing: level 2 needs KVM; \
         IGNORARIUM_VM_VMM is not set; point it at cloud-hypervisor-static v54.0; \
         IGNORARIUM_VM_KERNEL is not set; point it at vmlinux-x86_64 ch-release-v7.2.8-20261009; \
         IGNORARIUM_VM_AGENT is not set; point it at a static build of ignorarium-guest; \
         mkfs.ext4 cannot run: No such file or directory (os error 2); \
         debugfs cannot run: No such file or directory (os error 2)",
        kvm.display()
    );
    assert_eq!(b.probe(), Probe::Unavailable(want));
}

#[test]
fn probe_checks_both_pins() {
    let d = TempDir::new().unwrap();
    let (x, agent) = (d.path().join("x"), d.path().join("agent"));
    write_exec(&x, "x");
    fs::write(&agent, static_elf()).unwrap();
    let b = with_kvm(Path::new("/dev/null"), &x, &x, &agent);
    let found = sha256_file(&x).unwrap();
    let pin = |name: &str, sha: &str| {
        format!(
            "{name} at {}: expected sha256 {sha}, found {found}",
            x.display()
        )
    };
    assert_eq!(
        faults(&b),
        [
            pin("cloud-hypervisor-static v54.0", VMM.sha256),
            pin("vmlinux-x86_64 ch-release-v7.2.8-20261009", KERNEL.sha256),
        ]
    );
    assert!(VMM.sha256.starts_with("4f4f98a3") && KERNEL.sha256.starts_with("edfeb8a8"));
}

#[test]
fn probe_needs_a_vmm_it_can_run() {
    let d = TempDir::new().unwrap();
    let (vmm, agent) = (d.path().join("vmm"), d.path().join("agent"));
    fs::write(&vmm, "x").unwrap();
    fs::write(&agent, static_elf()).unwrap();
    let vmm_fault = |mode: u32| {
        fs::set_permissions(&vmm, Permissions::from_mode(mode)).unwrap();
        faults(&with_kvm(Path::new("/dev/null"), &vmm, &vmm, &agent)).remove(0)
    };
    let not_run = format!("{} has mode 644, so it cannot run", vmm.display());
    assert_eq!(vmm_fault(0o644), not_run);
    // Any execute bit lets the pin check read on.
    for mode in [0o744, 0o654, 0o645] {
        assert!(vmm_fault(mode).contains("expected sha256"), "{mode:o}");
    }
}

#[test]
fn the_defaults_size_each_row_s_machine() {
    let b = Level2::with("v".into(), "k".into(), "a".into());
    assert_eq!(
        (&b.kvm, &b.vmm, &b.kernel, &b.agent),
        (
            &PathBuf::from("/dev/kvm"),
            &PathBuf::from("v"),
            &PathBuf::from("k"),
            &PathBuf::from("a")
        )
    );
    assert_eq!((b.cpus, b.mem_mib, b.out_mib), (1, 1024, 256));
    let secs = Duration::from_secs;
    assert_eq!(
        (b.boot_timeout, b.exit_timeout, b.report_slack),
        (secs(30), secs(10), secs(30))
    );
}

#[test]
fn level2_cpus_is_the_vm_vcpu_count() {
    let mut b = Level2::with(PathBuf::new(), PathBuf::new(), PathBuf::new());
    assert_eq!(b.cpus(), 1);
    b.cpus = 3;
    assert_eq!(b.cpus(), 3);
}

#[test]
fn probe_refuses_a_dynamic_agent() {
    let d = TempDir::new().unwrap();
    let (x, not_elf) = (d.path().join("x"), d.path().join("not-elf"));
    fs::write(&x, "x").unwrap();
    fs::write(&not_elf, "#!/bin/sh\n").unwrap();
    let agent_fault = |agent: &Path| {
        let b = with_kvm(Path::new("/dev/null"), &x, &x, agent);
        faults(&b).pop().unwrap()
    };
    assert_eq!(
        agent_fault(Path::new("/bin/sh")),
        "/bin/sh is linked dynamically, so it cannot run as the guest's /init"
    );
    assert_eq!(
        agent_fault(&not_elf),
        format!("{}: not a 64-bit little-endian ELF file", not_elf.display())
    );
}

#[test]
fn probe_names_files_it_cannot_read() {
    let d = TempDir::new().unwrap();
    let missing = d.path().join("missing");
    let b = with_kvm(Path::new("/dev/null"), &missing, &missing, &missing);
    let want = format!(
        "{}: No such file or directory (os error 2)",
        missing.display()
    );
    assert_eq!(faults(&b), [want.clone(), want.clone(), want]);
}

#[test]
fn a_kvm_device_that_cannot_open_is_named() {
    let d = TempDir::new().unwrap();
    let kvm = d.path().join("kvm");
    fs::write(&kvm, "").unwrap();
    fs::set_permissions(&kvm, Permissions::from_mode(0o000)).unwrap();
    if File::options().read(true).write(true).open(&kvm).is_ok() {
        println!("skipped: this user opens a mode-000 file");
        return;
    }
    let b = with_kvm(&kvm, Path::new(""), Path::new(""), Path::new(""));
    let want = format!(
        "cannot open {}: Permission denied (os error 13)",
        kvm.display()
    );
    assert_eq!(faults(&b)[0], want);
}

#[test]
fn launcher_reads_the_hypervisor_version() {
    let d = TempDir::new().unwrap();
    let vmm = d.path().join("vmm");
    let prints = |line: &str| {
        let text = format!("#!/bin/sh\n[ \"$1\" = --version ] && echo '{line}'\necho more\n");
        write_exec(&vmm, &text);
    };
    let launcher = || Level2::with(vmm.clone(), PathBuf::new(), PathBuf::new());
    prints("cloud-hypervisor v54.0");
    let b = launcher();
    assert_eq!(b.launcher(), "cloud-hypervisor v54.0");
    // Read once: the same backend keeps its first answer.
    prints("cloud-hypervisor v55.1");
    assert_eq!(b.launcher(), "cloud-hypervisor v54.0");
    assert_eq!(launcher().launcher(), "cloud-hypervisor v55.1");
    prints("qemu 9.2");
    assert_eq!(launcher().launcher(), "cloud-hypervisor unknown");
    let missing = Level2::with(d.path().join("none"), PathBuf::new(), PathBuf::new());
    assert_eq!(missing.launcher(), "cloud-hypervisor unknown");
}

#[test]
fn guest_job_maps_paths_and_argv() {
    let d = TempDir::new().unwrap();
    let payload = d.path().join("payload");
    fs::create_dir(&payload).unwrap();
    let run = payload.join("run");
    write_exec(&run, "#!/bin/sh\n");
    let job = RowJob {
        argv: vec![
            run.to_string_lossy().into(),
            "--condition".into(),
            "a".into(),
        ],
        env: vec![("PATH".into(), "/usr/bin".into())],
        input_dir: d.path().join("in"),
        output_dir: d.path().join("out"),
        seed: [7; 32],
        wall_timeout: Duration::from_secs(5),
        rootfs: Some(d.path().join("root")),
        memory_mib: Some(64),
    };
    let h = guest_job(&job).unwrap();
    assert_eq!(h.argv, ["/payload/run", "--condition", "a"]);
    assert_eq!(h.payload_dir, payload);
    let env: Vec<(&str, &str)> = h.env.iter().map(|(k, v)| (&k[..], &v[..])).collect();
    assert_eq!(
        env,
        [
            ("PATH", "/usr/bin"),
            ("IGNORARIUM_IN", "/in"),
            ("IGNORARIUM_OUT", "/out")
        ]
    );
    assert_eq!(
        (&h.input_dir, &h.output_dir, h.seed, h.wall_timeout),
        (&job.input_dir, &job.output_dir, job.seed, job.wall_timeout)
    );
    assert_eq!(h.data_limit, Some(64 << 20));
    let uncapped = RowJob {
        memory_mib: None,
        ..job.clone()
    };
    assert_eq!(guest_job(&uncapped).unwrap().data_limit, None);
    let refused = |argv: Vec<String>| {
        guest_job(&RowJob {
            argv,
            ..job.clone()
        })
        .err()
        .unwrap()
    };
    assert_eq!(refused(vec![]), "empty argv");
    assert_eq!(refused(vec!["/".into()]), "payload / names no file");
    let missing = payload.join("missing");
    assert_eq!(
        refused(vec![missing.to_string_lossy().into()]),
        format!(
            "payload {}: No such file or directory (os error 2)",
            missing.display()
        )
    );
    // The guest runs the payload as another user, so it needs other-execute.
    let at = |mode: u32| {
        fs::set_permissions(&run, Permissions::from_mode(mode)).unwrap();
        guest_job(&job)
    };
    assert_eq!(
        at(0o754).err().unwrap(),
        format!(
            "payload {} has mode 754, which the guest's unprivileged user cannot run",
            run.display()
        )
    );
    assert!(at(0o755).is_ok());
}

fn passing() -> proto::SelfTest {
    proto::SelfTest {
        only_loopback: true,
        route_blocked: true,
        host_path_blocked: true,
        io_uring_errno: Some(libc::EPERM),
    }
}

fn mapped(connect: bool, vsock: bool) -> SelfTest {
    SelfTest {
        connect_blocked: connect,
        vsock_blocked: vsock,
        io_uring_blocked: true,
        vsock_absent: false,
        io_uring_absent: false,
    }
}

#[test]
fn selftest_of_maps_the_guest_checks() {
    let t = selftest_of(&passing());
    assert_eq!(t, mapped(true, true));
    assert!(t.ok());
    let with = |f: fn(&mut proto::SelfTest)| {
        let mut g = passing();
        f(&mut g);
        selftest_of(&g).failed()
    };
    assert_eq!(with(|g| g.route_blocked = false), ["connect"]);
    assert_eq!(with(|g| g.only_loopback = false), ["connect"]);
    assert_eq!(with(|g| g.host_path_blocked = false), ["vsock"]);
    // io_uring, as level 1 reads the same call.
    let io_uring = |errno: Option<i32>| {
        selftest_of(&proto::SelfTest {
            io_uring_errno: errno,
            ..passing()
        })
    };
    let t = io_uring(Some(libc::ENOSYS));
    assert!(t.io_uring_blocked && t.io_uring_absent);
    assert_eq!(t.io_uring_state(), "absent");
    assert_eq!(io_uring(Some(libc::EPERM)).io_uring_state(), "denied");
    for open in [None, Some(libc::EACCES)] {
        assert_eq!(io_uring(open).failed(), ["io_uring"], "{open:?}");
    }
}

#[test]
fn the_guest_and_the_box_agree_on_a_passing_self_test() {
    // The guest decides whether the payload runs; the box decides whether the row stands.
    let errnos = [
        libc::EPERM,
        libc::ENOSYS,
        libc::EAFNOSUPPORT,
        libc::EACCES,
        libc::EINVAL,
    ];
    for io_uring_errno in errnos.map(Some).into_iter().chain([None]) {
        for host_path_blocked in [true, false] {
            let g = proto::SelfTest {
                io_uring_errno,
                host_path_blocked,
                ..passing()
            };
            assert_eq!(g.ok(), selftest_of(&g).ok(), "{g:?}");
        }
    }
}
fn ran(end: Option<End>, selftest: proto::SelfTest) -> Result<VmOutcome, VmError> {
    Ok(VmOutcome {
        report: Report {
            selftest,
            clock_unix_nanos: 1,
            end,
            payload_wall_ns: 2,
            cpu_seconds: 0.75,
            output_hash: "h".into(),
            pack_refused: None,
        },
        output_hash: "h".into(),
        links: vec![],
        timings: Timings::default(),
    })
}

/// The result, then the exit, CPU seconds, whether the CPU is unknown, whether the box ended
/// the row, the self-test and the output hash it maps to.
type Case = (
    Result<VmOutcome, VmError>,
    Exit,
    f64,
    bool,
    bool,
    Option<SelfTest>,
    &'static str,
);

#[test]
fn outcome_of_maps_every_guest_ending() {
    const NO_REPORT: &str = "the guest sent no report, so the payload's CPU time is unknown";
    let infra = |s: &str| Exit::Infra(s.into());
    let no_route = proto::SelfTest {
        route_blocked: false,
        ..passing()
    };
    let refused = |end: End| {
        let mut res = ran(Some(end), passing());
        if let Ok(o) = &mut res {
            o.report.pack_refused = Some("too big".into());
            o.output_hash = String::new();
        }
        res
    };
    let io = |kind: io::ErrorKind| Err(VmError::Io(io::Error::new(kind, "x")));
    let ok = Some(mapped(true, true));
    let table: Vec<Case> = vec![
        (
            ran(Some(End::Code(0)), passing()),
            Exit::Ok,
            0.75,
            false,
            false,
            ok,
            "h",
        ),
        (
            ran(Some(End::Code(3)), passing()),
            Exit::Payload(3),
            0.75,
            false,
            false,
            ok,
            "h",
        ),
        (
            ran(Some(End::Signal(9)), passing()),
            Exit::Payload(137),
            0.75,
            false,
            false,
            ok,
            "h",
        ),
        (
            ran(Some(End::Timeout), passing()),
            Exit::Payload(TIMEOUT_CODE),
            0.75,
            false,
            true,
            ok,
            "h",
        ),
        (
            ran(None, no_route),
            infra("guest self-test failed: connect was not blocked"),
            0.75,
            false,
            false,
            Some(mapped(false, true)),
            "",
        ),
        (
            ran(None, passing()),
            infra("the guest ran no payload"),
            0.75,
            false,
            false,
            ok,
            "",
        ),
        (
            refused(End::Code(0)),
            Exit::Void("cannot pack output: too big".into()),
            0.75,
            false,
            false,
            ok,
            "",
        ),
        (
            refused(End::Code(3)),
            Exit::Payload(3),
            0.75,
            false,
            false,
            ok,
            "",
        ),
        (
            Err(VmError::Boot("b".into())),
            infra("boot failed: b"),
            0.0,
            false,
            false,
            None,
            "",
        ),
        (
            Err(VmError::Stopped),
            Exit::Stopped,
            0.0,
            false,
            true,
            None,
            "",
        ),
        (
            Err(VmError::Timeout("t".into())),
            infra("timed out: t"),
            0.0,
            true,
            true,
            None,
            "",
        ),
        (
            Err(VmError::Guest("g".into())),
            infra("guest error: g"),
            0.0,
            true,
            false,
            None,
            "",
        ),
        (
            Err(VmError::Mismatch {
                guest: "a".into(),
                host: "b".into(),
            }),
            infra("output hash mismatch: guest said a, host computed b"),
            0.0,
            true,
            false,
            None,
            "",
        ),
        (
            io(io::ErrorKind::InvalidInput),
            infra("host I/O error: x"),
            0.0,
            false,
            false,
            None,
            "",
        ),
        (
            io(io::ErrorKind::Other),
            infra("host I/O error: x"),
            0.0,
            true,
            false,
            None,
            "",
        ),
    ];
    for (i, (res, exit, cpu, unknown, ended, selftest, hash)) in table.into_iter().enumerate() {
        let o = outcome_of(res, 2.5, 1);
        assert_eq!(o.exit, exit, "case {i}");
        assert_eq!(o.cpu_seconds, cpu, "case {i}");
        assert_eq!(
            o.cpu_unknown.as_deref(),
            unknown.then_some(NO_REPORT),
            "case {i}"
        );
        assert_eq!(o.ended, ended, "case {i}");
        assert_eq!(o.selftest, selftest, "case {i}");
        assert_eq!(o.output_hash, hash, "case {i}");
        assert_eq!(o.wall_seconds, 2.5, "case {i}");
        assert_eq!(o.output_links, [], "case {i}");
    }
}

#[test]
fn outcome_of_keeps_the_output_links_with_the_hash() {
    let linked = |end: End, refused: bool| {
        let mut res = ran(Some(end), passing());
        if let Ok(o) = &mut res {
            let link = |path: &[u8], target: &[u8]| Link {
                path: path.into(),
                target: target.into(),
            };
            o.links = vec![link(b"latest", b"values.json"), link(b"sub/up", b"../x")];
            if refused {
                o.report.pack_refused = Some("too big".into());
            }
        }
        outcome_of(res, 2.5, 1).output_links
    };
    let rows = [
        tree::link_row(b"latest", b"values.json"),
        tree::link_row(b"sub/up", b"../x"),
    ];
    assert_eq!(linked(End::Code(0), false), rows);
    assert_eq!(linked(End::Code(3), false), rows);
    // No hash, no links: they belong to the output the hash covers.
    assert_eq!(linked(End::Code(0), true), []);
    assert_eq!(linked(End::Code(3), true), []);
}

fn with_cpu(cpu_seconds: f64) -> Result<VmOutcome, VmError> {
    let mut res = ran(Some(End::Code(0)), passing());
    if let Ok(o) = &mut res {
        o.report.cpu_seconds = cpu_seconds;
    }
    res
}

#[test]
fn outcome_of_refuses_a_report_no_guest_can_mean() {
    // Two vCPUs over a 2.5 s row use at most 5 CPU seconds.
    for cpu in [0.0, 5.0] {
        assert_eq!(outcome_of(with_cpu(cpu), 2.5, 2).exit, Exit::Ok, "{cpu}");
    }
    for cpu in [-0.5, 5.001, f64::NAN, f64::INFINITY] {
        let o = outcome_of(with_cpu(cpu), 2.5, 2);
        let why =
            format!("the guest reported {cpu} CPU seconds, outside 0 to 5 for 2 vCPUs over 2.5 s");
        assert_eq!(o.exit, Exit::Infra(why), "{cpu}");
        assert_eq!((o.cpu_seconds, o.ended), (0.0, false));
        assert_eq!(
            o.cpu_unknown.as_deref(),
            Some(
                "the guest reported a CPU time it cannot have used, so the payload's CPU time is unknown"
            )
        );
        assert_eq!(o.output_hash, "");
    }
    // A process exits with a code of 0 to 255, or dies of a signal of 1 to 127.
    let ended = |end: End| outcome_of(ran(Some(end), passing()), 2.5, 1);
    for (end, exit) in [
        (End::Code(255), Exit::Payload(255)),
        (End::Signal(1), Exit::Payload(129)),
        (End::Signal(127), Exit::Payload(255)),
    ] {
        assert_eq!(ended(end).exit, exit);
    }
    for (end, what) in [
        (End::Code(-7), "exit code -7"),
        (End::Code(256), "exit code 256"),
        (End::Signal(0), "signal 0"),
        (End::Signal(-128), "signal -128"),
        (End::Signal(128), "signal 128"),
        (End::Signal(i32::MAX), "signal 2147483647"),
    ] {
        let o = ended(end);
        assert_eq!(o.exit, Exit::Infra(format!("the guest reported {what}")));
        assert_eq!((o.output_hash.as_str(), o.cpu_seconds), ("", 0.75));
    }
}

#[test]
fn rootfs_disk_is_built_once() {
    let d = TempDir::new().unwrap();
    let root = d.path().join("root");
    fs::create_dir_all(root.join("usr/bin")).unwrap();
    fs::write(root.join("usr/bin/tool"), "x").unwrap();
    let disk = rootfs_disk(&root).unwrap().unwrap();
    assert_eq!(disk, d.path().join("root.ext4"));
    // The ext4 superblock starts at byte 1024; its magic 0xEF53 sits 56 bytes in.
    assert_eq!(fs::read(&disk).unwrap()[1080..1082], [0x53, 0xef]);
    let built = fs::metadata(&disk).unwrap().modified().unwrap();
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(rootfs_disk(&root).unwrap(), Ok(disk.clone()));
    assert_eq!(fs::metadata(&disk).unwrap().modified().unwrap(), built);
    let names = || {
        let mut v: Vec<String> = fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    };
    assert_eq!(names(), ["root", "root.ext4"]);
    // A failed build leaves nothing beside the root.
    assert!(rootfs_disk(&d.path().join("absent")).is_err());
    assert_eq!(names(), ["root", "root.ext4"]);
    // A file mkfs.ext4 cannot read is a fact of the image, found before mkfs runs.
    let bad = d.path().join("bad");
    fs::create_dir_all(bad.join("etc")).unwrap();
    for f in ["etc/a", "etc/shadow", "etc/z"] {
        fs::write(bad.join(f), "x").unwrap();
    }
    for f in ["etc/shadow", "etc/z"] {
        fs::set_permissions(bad.join(f), Permissions::from_mode(0o200)).unwrap();
    }
    if File::open(bad.join("etc/shadow")).is_ok() {
        println!("skipped the unreadable file: this user reads a mode-200 file");
        return;
    }
    let why = "etc/shadow has mode 200, so mkfs.ext4 cannot read it";
    assert_eq!(rootfs_disk(&bad).unwrap(), Err(why.to_string()));
    assert_eq!(names(), ["bad", "root", "root.ext4"]);
}

/// mkfs.ext4 fails after the temp disk exists: the failed build must remove it.
#[test]
fn a_failed_mkfs_leaves_no_temp_disk() {
    const NAME: &str = "a_failed_mkfs_leaves_no_temp_disk";
    let d = TempDir::new().unwrap();
    if !is_child(NAME) {
        // An empty PATH: mkfs.ext4 cannot start, once the temp disk is made.
        assert!(in_child(NAME, d.path(), &[("PATH", d.path().as_os_str())]));
        return;
    }
    let root = d.path().join("root");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("f"), "x").unwrap();
    let e = rootfs_disk(&root).unwrap_err();
    assert!(e.to_string().contains("mkfs.ext4"), "{e}");
    let names: Vec<_> = fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["root"]);
}

/// Levels 0 and 1 hash an output folder with `output_hash`; level 2's guest and host hash the
/// pack stream with the guest crate's rows. The four agree, links included, and the folder the
/// host unpacks, with the rows of the links it never makes, hashes the same.
#[test]
fn the_output_hash_is_one_function_at_every_level() {
    let d = TempDir::new().unwrap();
    let src = d.path();
    fs::write(src.join("a.txt"), "hi").unwrap();
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("sub/b"), vec![7u8; 5000]).unwrap();
    fs::create_dir(src.join("e")).unwrap();
    for (link, target) in [("latest", "/tmp/x"), ("up", "../x"), ("rel", "a.txt")] {
        std::os::unix::fs::symlink(target, src.join(link)).unwrap();
    }
    let fifo = std::process::Command::new("mkfifo")
        .arg(src.join("p"))
        .status()
        .unwrap();
    assert!(fifo.success());
    // The last two names both read `!bytes:ff`.
    for name in [&b"x\xff"[..], b"!bytes:ff", b"\xff"] {
        fs::write(src.join(OsStr::from_bytes(name)), name).unwrap();
    }
    let want = output_hash(src).unwrap();
    assert_eq!(tree::tree_hash(src).unwrap(), want);
    let mut stream = Vec::new();
    let packed = pack::write(src, &mut stream, u64::MAX).unwrap();
    let out = TempDir::new().unwrap();
    let unpacked = pack::unpack(&mut stream.as_slice(), out.path(), u64::MAX).unwrap();
    assert_eq!((&packed.hash, &unpacked.hash), (&want, &want));
    assert_ne!(output_hash(out.path()).unwrap(), want);
    let mut rows = tree::tree_rows(out.path()).unwrap();
    let links = unpacked.links.iter();
    rows.extend(links.map(|l| tree::link_row(&l.path, &l.target)));
    assert_eq!(rows.len(), 9);
    assert_eq!(tree::hash_rows(&rows).unwrap(), want);
}

/// Records each `started` call with the process group of the pid at that moment.
#[derive(Default)]
struct Leaders(RefCell<Vec<(u32, u32)>>);

impl Watch for Leaders {
    fn tick(&self) -> bool {
        true
    }
    fn started(&self, pid: u32) {
        let group = getpgid(Pid::from_raw(pid as i32)).unwrap();
        let group = group.as_raw_nonzero().get() as u32;
        self.0.borrow_mut().push((pid, group));
    }
}

#[test]
fn a_row_without_an_environment_is_infra_before_any_vm() {
    let d = TempDir::new().unwrap();
    let b = Level2::with(PathBuf::new(), PathBuf::new(), PathBuf::new());
    let job = RowJob {
        argv: vec!["/bin/true".into()],
        input_dir: d.path().into(),
        output_dir: d.path().into(),
        wall_timeout: LONG,
        rootfs: None,
        ..RowJob::default()
    };
    assert_eq!(b.level(), 2);
    let watch = Leaders::default();
    let (o, timings) = b.run_timed(&job, &watch);
    assert_eq!(
        o.exit,
        Exit::Infra("level 2 runs rows in an environment image; this study has none".into())
    );
    assert!(watch.0.borrow().is_empty());
    assert!(timings.is_none());
    assert_eq!((o.cpu_seconds, o.output_hash.as_str()), (0.0, ""));
    assert_eq!((o.selftest, o.cpu_unknown), (None, None));
    assert_eq!(b.run_row(&job, &watch).unwrap().exit, o.exit);
}

#[test]
fn a_stop_before_the_vm_skips_what_is_left() {
    let d = TempDir::new().unwrap();
    let (root, payload) = (d.path().join("root"), d.path().join("payload"));
    for dir in [&root, &payload] {
        fs::create_dir(dir).unwrap();
    }
    write_exec(&payload.join("run"), "#!/bin/sh\n");
    let job = RowJob {
        argv: vec![payload.join("run").to_string_lossy().into()],
        input_dir: d.path().into(),
        output_dir: d.path().into(),
        wall_timeout: LONG,
        rootfs: Some(root.clone()),
        ..RowJob::default()
    };
    // No agent file: a row that went on past the stop would fail on it instead.
    let b = Level2::with(PathBuf::new(), PathBuf::new(), PathBuf::new());
    let disk = d.path().join("root.ext4");
    assert_eq!(b.run_row(&job, &|| false).unwrap().exit, Exit::Stopped);
    assert!(!disk.exists(), "the disk was built after a stop");
    // A stop while the disk is built: the disk stays for the next row.
    let ticks = Cell::new(0);
    let second_stops = || {
        ticks.set(ticks.get() + 1);
        ticks.get() < 2
    };
    assert_eq!(b.run_row(&job, &second_stops).unwrap().exit, Exit::Stopped);
    assert!(disk.is_file());
}

/// A backend whose VMM writes its arguments to `<dir>/args` and exits at once, so every row
/// ends before boot, and a row for it: environment root, payload and folders, all in `dir`.
fn fake_box(dir: &Path) -> (Level2, RowJob) {
    for f in ["root", "in", "out", "payload"] {
        fs::create_dir(dir.join(f)).unwrap();
    }
    for f in ["in", "payload"] {
        fs::set_permissions(dir.join(f), Permissions::from_mode(0o755)).unwrap();
    }
    write_exec(&dir.join("payload/run"), "#!/bin/sh\n");
    let vmm = dir.join("vmm");
    let args = dir.join("args");
    write_exec(
        &vmm,
        &format!("#!/bin/sh\necho \"$@\" > {}\n", args.display()),
    );
    fs::write(dir.join("agent"), static_elf()).unwrap();
    let b = Level2::with(vmm, dir.join("kernel"), dir.join("agent"));
    let job = RowJob {
        argv: vec![dir.join("payload/run").to_string_lossy().into()],
        input_dir: dir.join("in"),
        output_dir: dir.join("out"),
        wall_timeout: LONG,
        rootfs: Some(dir.join("root")),
        ..RowJob::default()
    };
    (b, job)
}

#[test]
fn a_disk_that_cannot_be_built_names_the_environment_by_its_digest() {
    // The reason can reach a signed spend record, so it holds no path of the box's home.
    let d = TempDir::new().unwrap();
    let (b, mut job) = fake_box(d.path());
    let digest = "cd".repeat(32);
    job.rootfs = Some(d.path().join("store/rootfs").join(&digest));
    let o = b.run_row(&job, &|| true).unwrap();
    let why = format!(
        "cannot build the disk of environment {digest}: No such file or directory (os error 2)"
    );
    assert_eq!(o.exit, Exit::Infra(why));
}

#[test]
fn the_vmm_s_cpu_is_box_spend_in_the_meter_and_the_records_not_in_the_row() {
    let lab = common::lab();
    let id = "vmm-cpu";
    common::plain(&lab, id);
    let work = common::work_of(&lab.home, id);
    let mut plan = runner::prepare_with(&lab.home, id, &work, vec![Box::new(Level0)]).unwrap();
    let d = TempDir::new().unwrap();
    let (b, job) = fake_box(d.path());
    // The VMM burns half a second of CPU, then exits before a guest connects.
    let burn = "#!/bin/sh\nexec python3 -c 'import time\nt = time.process_time()\n\
                while time.process_time() - t < 0.5: pass'\n";
    write_exec(&b.vmm, burn);
    (plan.backend, plan.rootfs) = (Box::new(b), job.rootfs);
    // The boot fails; before the retry, the probe finds the fake VMM off its pin.
    let status = runner::run(&plan).unwrap();
    let paused = matches!(&status, RunStatus::Paused(w) if w.starts_with("backend unavailable: "));
    assert!(paused, "{status:?}");
    let spend = common::kind_of(&lab, id, "spend");
    assert_eq!(spend.len(), 1);
    assert_eq!(spend[0].body["cpu_seconds"], 0.0);
    let vmm = spend[0].body["box_cpu_seconds"].as_f64().unwrap();
    assert!((0.45..2.0).contains(&vmm), "{vmm}");
    assert_eq!(plan.meter.sample().unwrap().cpu_seconds, vmm);
    // A later session starts from the records, which hold it.
    let all = common::records_of(&lab, id);
    let spent = runner::progress(&lab.home, id, &all).unwrap().cost;
    assert_eq!(spent.cpu_seconds, vmm);
}

fn boot_failed(exit: &Exit) -> bool {
    matches!(exit, Exit::Infra(why) if why.starts_with("boot failed: the VMM exited (exit status: 0)"))
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn a_row_removes_its_scratch_folder_and_a_dead_box_s() {
    const NAME: &str = "a_row_removes_its_scratch_folder_and_a_dead_box_s";
    if !is_child(NAME) {
        let d = TempDir::new().unwrap();
        let tmp = d.path().join("tmp");
        fs::create_dir(&tmp).unwrap();
        assert!(in_child(NAME, d.path(), &[("TMPDIR", tmp.as_os_str())]));
        return;
    }
    let dir = std::env::current_dir().unwrap();
    let tmp = dir.join("tmp");
    assert_eq!(std::env::temp_dir(), tmp);
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let dead = child.id();
    child.wait().unwrap();
    // A dead box's scratch, one of a live process (PID 1), and a folder that is not scratch.
    for name in [
        format!("ignorarium-vm-{dead}-0"),
        "ignorarium-vm-1-0".into(),
        "other".into(),
    ] {
        fs::create_dir_all(tmp.join(name).join("work")).unwrap();
    }
    // A link with a dead box's scratch name is not a folder of this user: it stays.
    let link = format!("ignorarium-vm-{dead}-1");
    std::os::unix::fs::symlink(tmp.join("other"), tmp.join(&link)).unwrap();
    let (b, job) = fake_box(&dir);
    let o = b.run_row(&job, &|| true).unwrap();
    assert!(boot_failed(&o.exit), "{o:?}");
    // The row ran in its own scratch folder, under the temp folder.
    let args = fs::read_to_string(dir.join("args")).unwrap();
    let initramfs = args
        .split(' ')
        .skip_while(|a| *a != "--initramfs")
        .nth(1)
        .unwrap();
    let scratch = tmp.join(format!("ignorarium-vm-{}-0", std::process::id()));
    assert_eq!(Path::new(initramfs), scratch.join("initramfs.cpio"));
    let mut kept = vec!["ignorarium-vm-1-0".to_string(), link, "other".into()];
    kept.sort();
    assert_eq!(names(&tmp), kept);
}

#[test]
fn the_agent_is_named_by_the_sha256_of_the_bytes_the_rows_boot() {
    let d = TempDir::new().unwrap();
    let (mut b, job) = fake_box(d.path());
    let agent = b.agent.clone();
    b.agent = d.path().join("later");
    assert_eq!(b.agent_sha256(), None);
    fs::copy(&agent, &b.agent).unwrap();
    let want = sha256_file(&agent).unwrap();
    assert_eq!(b.agent_sha256(), Some(want.clone()));
    // Read once with the initramfs: a new file is not what the rows boot.
    fs::write(&b.agent, b"another build").unwrap();
    assert!(boot_failed(&b.run_row(&job, &|| true).unwrap().exit));
    assert_eq!(b.agent_sha256(), Some(want));
}

#[test]
fn a_failed_agent_read_is_tried_again_next_row() {
    let d = TempDir::new().unwrap();
    let (mut b, job) = fake_box(d.path());
    b.agent = d.path().join("later");
    let o = b.run_row(&job, &|| true).unwrap();
    let why = format!("cannot read the agent {}: ", b.agent.display());
    assert!(
        matches!(&o.exit, Exit::Infra(w) if w.starts_with(&why)),
        "{o:?}"
    );
    fs::write(&b.agent, static_elf()).unwrap();
    let o = b.run_row(&job, &|| true).unwrap();
    assert!(boot_failed(&o.exit), "{o:?}");
}

#[test]
fn a_required_level2_turns_a_skip_into_a_failure() {
    const NAME: &str = "a_required_level2_turns_a_skip_into_a_failure";
    if is_child(NAME) {
        assert!(ready().is_none());
        return;
    }
    let d = TempDir::new().unwrap();
    let unset = OsStr::new("");
    let mut envs: Vec<(&str, &OsStr)> = VARS.iter().map(|v| (*v, unset)).collect();
    envs.push((REQUIRE_LEVEL2, OsStr::new("0")));
    assert!(in_child(NAME, d.path(), &envs));
    envs.pop();
    envs.push((REQUIRE_LEVEL2, OsStr::new("1")));
    assert!(!in_child(NAME, d.path(), &envs));
}

#[test]
fn a_level2_row_runs_and_hashes_its_output() {
    let Some(r) = ready() else { return };
    let (_d, job) = row(
        &r,
        "printf 42 > \"$IGNORARIUM_OUT/result\"; mkdir \"$IGNORARIUM_OUT/sub\"; \
         cat \"$IGNORARIUM_IN/seen\" > \"$IGNORARIUM_OUT/sub/copy\"",
        LONG,
    );
    let o = r.level2.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Ok, "{o:?}");
    assert!(o.selftest.expect("a self-test").ok());
    assert_eq!(o.cpu_unknown, None);
    assert_eq!(read(&job, "result"), "42");
    assert_eq!(read(&job, "sub/copy"), "input");
    assert_eq!(o.output_hash, output_hash(&job.output_dir).unwrap());
    assert!(o.wall_seconds > 0.0 && o.wall_seconds < LONG.as_secs_f64());
}

#[test]
fn the_payload_sees_guest_paths() {
    let Some(r) = ready() else { return };
    let (_d, job) = row(
        &r,
        "printf '%s %s %s %s %s' \"$0\" \"$IGNORARIUM_IN\" \"$IGNORARIUM_OUT\" \"$1\" \"$2\" \
         > \"$IGNORARIUM_OUT/paths\"; echo rel > relative",
        LONG,
    );
    let o = r.level2.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Ok, "{o:?}");
    assert_eq!(read(&job, "paths"), "/payload/run /in /out --condition a");
    // The payload starts in its output folder, as at level 1.
    assert_eq!(read(&job, "relative"), "rel\n");
}

#[test]
fn exit_codes_and_the_wall_timeout_pass_through() {
    let Some(r) = ready() else { return };
    let (_d, job) = row(&r, "exit 3", LONG);
    assert_eq!(
        r.level2.run_row(&job, &|| true).unwrap().exit,
        Exit::Payload(3)
    );
    let (_d, job) = row(&r, "sleep 30", Duration::from_secs(2));
    let t = Instant::now();
    let o = r.level2.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Payload(TIMEOUT_CODE), "{o:?}");
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
}

/// True when no process is left in the group led by `pid`.
fn group_gone(pid: u32) -> bool {
    let pid = Pid::from_raw(pid as i32).unwrap();
    test_kill_process_group(pid).is_err_and(|e| e == rustix::io::Errno::SRCH)
}

#[test]
fn started_names_one_group() {
    let Some(r) = ready() else { return };
    let (_d, job) = row(&r, "true", LONG);
    let watch = Leaders::default();
    let o = r.level2.run_row(&job, &watch).unwrap();
    assert_eq!(o.exit, Exit::Ok, "{o:?}");
    let seen = watch.0.borrow();
    assert_eq!(seen.len(), 1);
    let (pid, group) = seen[0];
    assert_eq!(group, pid);
    assert!(group_gone(pid));
}

/// Says stop half a second after the guest connects: the host removes its listening
/// socket then, so the payload is most likely running by the stop. No channel reaches the
/// host from the payload, so no marker can prove it; the host's path while the guest runs
/// is pinned by the fake-VMM test `a_stop_while_the_guest_runs_kills_the_group`.
/// `started` can come before the kernel sets the VMM's arguments, so `tick` looks for the
/// socket path until it finds it.
#[derive(Default)]
struct StopOnceRunning {
    pid: Cell<Option<u32>>,
    listen: RefCell<Option<PathBuf>>,
    stop_at: Cell<Option<Instant>>,
}

/// The host's control socket, from the `--vsock` argument of process `pid`. `None` while
/// the process's arguments hold no `--vsock`.
fn control_socket(pid: u32) -> Option<PathBuf> {
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let mut args = cmdline.split(|&b| b == 0);
    args.position(|a| a == b"--vsock")?;
    let vsock = std::str::from_utf8(args.next()?).ok()?;
    let socket = vsock.split_once("socket=")?.1;
    Some(PathBuf::from(format!("{socket}_7201")))
}

impl Watch for StopOnceRunning {
    fn tick(&self) -> bool {
        if let Some(at) = self.stop_at.get() {
            return Instant::now() < at;
        }
        if self.listen.borrow().is_none() {
            let found = self.pid.get().and_then(control_socket);
            *self.listen.borrow_mut() = found;
        }
        if self.listen.borrow().as_ref().is_some_and(|p| !p.exists()) {
            self.stop_at
                .set(Some(Instant::now() + Duration::from_millis(500)));
        }
        true
    }

    fn started(&self, pid: u32) {
        self.pid.set(Some(pid));
    }
}

#[test]
fn the_stop_watch_reads_the_vmm_arguments_once_they_are_set() {
    // A VMM whose arguments the kernel has not set yet holds no `--vsock`, as this process.
    let watch = StopOnceRunning::default();
    watch.started(std::process::id());
    assert!(watch.tick());
    assert_eq!(*watch.listen.borrow(), None);
    assert_eq!(watch.stop_at.get(), None);
    // The compound command keeps `sh` from replacing itself with `sleep`.
    let mut child = std::process::Command::new("sh")
        .args(["-c", "sleep 10; exit 0", "sh", "--vsock"])
        .arg("cid=3,socket=/nowhere/vsock.sock")
        .spawn()
        .unwrap();
    let watch = StopOnceRunning::default();
    watch.started(child.id());
    let t = Instant::now();
    while watch.listen.borrow().is_none() {
        assert!(t.elapsed() < Duration::from_secs(5), "no --vsock seen");
        assert!(watch.tick());
    }
    let listen = watch.listen.borrow().clone();
    assert_eq!(listen, Some(PathBuf::from("/nowhere/vsock.sock_7201")));
    // That socket does not exist, as after the guest connects: the stop is due.
    assert!(watch.stop_at.get().is_some());
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn a_stop_kills_the_vm() {
    let Some(r) = ready() else { return };
    let (_d, job) = row(&r, "sleep 60", LONG);
    let watch = StopOnceRunning::default();
    let o = r.level2.run_row(&job, &watch).unwrap();
    let returned = Instant::now();
    assert_eq!(o.exit, Exit::Stopped, "{o:?}");
    let stop = watch.stop_at.get().expect("the guest never connected");
    assert!(returned.duration_since(stop) < Duration::from_secs(3));
    assert!(group_gone(watch.pid.get().unwrap()));
}

#[test]
fn cpu_comes_from_the_guest() {
    let Some(r) = ready() else { return };
    let busy = "exec python3 -c 'import time\nt = time.process_time()\n\
                while time.process_time() - t < 1: pass'";
    let (_d, job) = row(&r, busy, LONG);
    let o = r.level2.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Ok, "{o:?}");
    assert!((0.5..2.0).contains(&o.cpu_seconds), "{}", o.cpu_seconds);
    assert_eq!(o.cpu_unknown, None);
}

#[test]
fn filling_out_is_the_payloads_result() {
    let Some(mut r) = ready() else { return };
    r.level2.out_mib = 16;
    let (_d, job) = row(
        &r,
        "head -c 20M /dev/zero > \"$IGNORARIUM_OUT/big\" || exit 7",
        LONG,
    );
    let o = r.level2.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Payload(7), "{o:?}");
    assert_eq!(o.output_hash, output_hash(&job.output_dir).unwrap());
}

#[test]
fn the_vm_holds_the_cap_the_out_disk_and_the_guest() {
    let mut b = Level2::with(PathBuf::new(), PathBuf::new(), PathBuf::new());
    let job = |memory_mib| RowJob {
        memory_mib,
        ..RowJob::default()
    };
    // No declared memory: today's machine.
    assert_eq!(b.vm_mem_mib(&job(None)), 1024);
    // The cap, /out (half the 256 MiB output disk), /tmp (all of it) and the guest's own.
    assert_eq!(GUEST_MIB, 256);
    assert_eq!(b.vm_mem_mib(&job(Some(2048))), 2688);
    assert_eq!(b.vm_mem_mib(&job(Some(64))), 704);
    b.out_mib = 16;
    assert_eq!(b.vm_mem_mib(&job(Some(2048))), 2048 + 8 + 16 + 256);
    // /out of an odd disk takes a part of a MiB, which counts as a whole one.
    b.out_mib = 3;
    assert_eq!(b.vm_mem_mib(&job(Some(2048))), 2048 + 5 + 256);
    b.mem_mib = 4096;
    assert_eq!(b.vm_mem_mib(&job(None)), 4096);
    // No machine has that much; the host refuses the largest size there is.
    assert_eq!(b.vm_mem_mib(&job(Some(u32::MAX))), u32::MAX);
}

/// A level 2 row that runs `script` and declares `memory_mib`.
fn capped(r: &Ready, memory_mib: Option<u32>, script: &str) -> (TempDir, RowJob) {
    let (d, job) = row(r, script, LONG);
    (d, RowJob { memory_mib, ..job })
}

#[test]
fn an_absent_row_memory_keeps_todays_limits() {
    let Some(r) = ready() else { return };
    let (_d, job) = capped(&r, None, common::LIMITS);
    let o = r.level2.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Ok, "{o:?}");
    let limits = common::data_limit(&read(&job, "limits"));
    assert_eq!(limits, ["unlimited", "unlimited"]);
}

#[test]
fn the_cap_reaches_the_payload() {
    let Some(r) = ready() else { return };
    let (_d, job) = capped(&r, Some(256), common::LIMITS);
    let o = r.level2.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Ok, "{o:?}");
    let limits = common::data_limit(&read(&job, "limits"));
    assert_eq!(limits, ["268435456", "268435456"]);
}

#[test]
fn a_row_past_its_memory_fails_as_a_payload_failure() {
    let Some(r) = ready() else { return };
    let (_d, job) = capped(&r, Some(64), common::HOLD_100_MB);
    let o = r.level2.run_row(&job, &|| true).unwrap();
    assert!(common::failed_by_itself(&o.exit), "{o:?}");
    assert!(!o.ended);
}

#[test]
fn a_row_well_under_its_memory_runs_at_every_level() {
    let Some(r) = ready() else { return };
    let (_d, job) = capped(&r, Some(1024), common::HOLD_100_MB);
    let o = r.level2.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Ok, "{o:?}");
}

/// The value of `key` in `/proc/meminfo` text, in kB.
fn meminfo_kb(text: &str, key: &str) -> u64 {
    let line = text
        .lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'));
    let value = line.unwrap_or_else(|| panic!("no {key} in {text}"));
    value.trim().trim_end_matches("kB").trim().parse().unwrap()
}

#[test]
fn a_level2_vm_boots_with_the_declared_size() {
    let Some(r) = ready() else { return };
    let script = format!(
        "cat /proc/meminfo > \"$IGNORARIUM_OUT/meminfo\"; {}",
        common::LIMITS
    );
    let (_d, job) = capped(&r, Some(2048), &script);
    let size = r.level2.vm_mem_mib(&job);
    let o = r.level2.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Ok, "{o:?}");
    let total = meminfo_kb(&read(&job, "meminfo"), "MemTotal");
    println!("a {size} MiB guest: MemTotal {total} kB");
    // The kernel keeps some of the machine for itself, but the cap and both tmpfs fit.
    let held: u64 = (2048 + 128 + 256) << 10;
    assert!((held..u64::from(size) << 10).contains(&total), "{total} kB");
    let limits = common::data_limit(&read(&job, "limits"));
    assert_eq!(limits, ["2147483648", "2147483648"]);
}

#[test]
fn a_row_near_its_memory_with_full_tmpfs_runs_at_level_2() {
    let Some(r) = ready() else { return };
    // /tmp and /out full, then data 16 MiB under the 256 MiB cap. A guest without room for
    // all three kills the payload (137) or fails to report.
    let script = "cat /proc/meminfo > \"$IGNORARIUM_OUT/meminfo\"
        head -c 256M /dev/zero > /tmp/fill
        [ \"$(wc -c < /tmp/fill)\" -ge $((255 << 20)) ] || exit 8
        head -c 128M /dev/zero > \"$IGNORARIUM_OUT/fill\"
        dd if=/dev/zero of=/dev/null bs=240M count=1 2>/dev/null || exit 9";
    let (_d, job) = capped(&r, Some(256), script);
    let size = r.level2.vm_mem_mib(&job);
    assert_eq!(size, 256 + 128 + 256 + GUEST_MIB);
    let o = r.level2.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Ok, "{o:?}");
    let filled = fs::metadata(job.output_dir.join("fill")).unwrap().len();
    assert!(filled >= 127 << 20, "{filled}");
    let meminfo = read(&job, "meminfo");
    let (total, free) = (
        meminfo_kb(&meminfo, "MemTotal"),
        meminfo_kb(&meminfo, "MemAvailable"),
    );
    println!(
        "a {size} MiB guest at the payload's start: MemTotal {total} kB, MemAvailable {free} kB"
    );
}

#[test]
fn level1_and_level2_give_the_same_output_hash() {
    let Some(r) = ready() else { return };
    if common::skip_level1() {
        return;
    }
    let script = "printf data > data; mkdir sub; printf x > sub/x; \
                  ln -s /tmp/x latest; ln -s ../x up; mkfifo p";
    let (_d1, job1) = row(&r, script, LONG);
    let (_d2, job2) = row(&r, script, LONG);
    let box_exe = env!("CARGO_BIN_EXE_ignorarium");
    let level1 = Level1::with(Level1::new().bwrap, box_exe.into());
    let o1 = level1.run_row(&job1, &|| true).unwrap();
    let o2 = r.level2.run_row(&job2, &|| true).unwrap();
    assert_eq!(
        (&o1.exit, &o2.exit),
        (&Exit::Ok, &Exit::Ok),
        "{o1:?} {o2:?}"
    );
    assert_eq!(o1.output_hash, o2.output_hash);
    assert_eq!(o1.output_hash, output_hash(&job1.output_dir).unwrap());
    assert_eq!(o1.output_links, []);
    let links = [
        tree::link_row(b"latest", b"/tmp/x"),
        tree::link_row(b"up", b"../x"),
    ];
    assert_eq!(o2.output_links, links);
    // Level 2 records its links and makes none.
    for name in ["latest", "up"] {
        assert!(fs::symlink_metadata(job2.output_dir.join(name)).is_err());
    }
}

/// `t` with `from` replaced by `to`. `from` must be there, so a changed template fails here
/// instead of registering a study that differs from the one meant.
fn swap(t: String, from: &str, to: &str) -> String {
    assert!(t.contains(from), "the template has no {from:?}");
    t.replace(from, to)
}

/// Registers study `id` at `min_isolation`, `units` units per cluster, `pins.environment` =
/// `digest`, running `payload`; `edit` changes the spec text last.
fn register_pinned(
    lab: &common::Lab,
    id: &str,
    (min_isolation, units, digest): (u8, u32, &str),
    payload: &str,
    edit: impl Fn(String) -> String,
) {
    let spec = |t: String| {
        let t = swap(
            t,
            "min_isolation = 0",
            &format!("min_isolation = {min_isolation}"),
        );
        let t = swap(
            t,
            "units_per_cluster = 3",
            &format!("units_per_cluster = {units}"),
        );
        edit(swap(
            t,
            "environment = \"\"",
            &format!("environment = \"{digest}\""),
        ))
    };
    let before = [("data/example.txt", "calibration")];
    common::register_study(lab, id, spec, &before, payload);
}

/// A lab whose machine reaches level 2, so a study may ask for it.
fn level2_lab() -> common::Lab {
    let lab = common::lab();
    fs::write(lab.home.root.join("machine.toml"), "max_level = 2\n").unwrap();
    lab
}

/// A backend that is available at its level and runs no row.
struct Stub(u8);

impl Backend for Stub {
    fn level(&self) -> u8 {
        self.0
    }
    fn probe(&self) -> Probe {
        Probe::Available
    }
    fn cpus(&self) -> u32 {
        1
    }
    fn launcher(&self) -> String {
        "stub".into()
    }
    fn run_row(&self, _: &RowJob, _: &dyn Watch) -> io::Result<RowOutcome> {
        Err(io::Error::other("stub"))
    }
}

#[test]
fn prepare_refuses_level2_without_an_environment() {
    let lab = level2_lab();
    let all = || -> Vec<Box<dyn Backend>> {
        vec![Box::new(Level0), Box::new(Stub(1)), Box::new(Stub(2))]
    };
    for (id, min) in [("needs2", 2), ("needs1", 1), ("needs0", 0)] {
        let spec = |t| swap(t, "min_isolation = 0", &format!("min_isolation = {min}"));
        common::register_study(&lab, id, spec, &[], common::PAYLOAD);
    }
    let prepare =
        |id: &str| runner::prepare_with(&lab.home, id, &common::work_of(&lab.home, id), all());
    let err = prepare("needs2").err().unwrap();
    assert!(matches!(&err, RunError::Environment(_)), "{err}");
    assert_eq!(
        err.to_string(),
        "environment: level 2 runs rows in the study's environment image; \
         add env/ or set pins.environment"
    );
    // Refused before the schedule: no row can start.
    let schedule = |id: &str| lab.home.study_dir(id).join("schedule.json");
    assert!(!schedule("needs2").exists());
    let chosen = |id: &str| {
        let events = common::events_of(&lab.home.study_dir(id));
        (common::named(&events, "run.isolation").into_iter().cloned()).collect::<Vec<_>>()
    };
    assert!(chosen("needs2").is_empty());
    // A lower minimum runs at the highest level that needs no image, on the host root, and
    // one event says why level 2 did not run it.
    for id in ["needs1", "needs0"] {
        let plan = prepare(id).unwrap();
        assert_eq!(
            (plan.backend.level(), plan.rootfs, plan.environment),
            (1, None, "host".to_string()),
            "{id}"
        );
        assert!(schedule(id).exists());
        let events = chosen(id);
        assert_eq!(events.len(), 1, "{id}");
        assert_eq!(events[0]["level"], "warn");
        assert_eq!(
            events[0]["attrs"],
            json!({
                "level": 1,
                "skipped": [2],
                "reason": "level 2 runs rows in the study's environment image; \
                           the study has no env/ and no pins.environment"
            })
        );
    }
    // No level skipped, no event.
    let lower: Vec<Box<dyn Backend>> = vec![Box::new(Level0), Box::new(Stub(1))];
    let work = common::work_of(&lab.home, "needs1");
    runner::prepare_with(&lab.home, "needs1", &work, lower).unwrap();
    assert_eq!(chosen("needs1").len(), 1);
    // With level 2 alone, nothing is left to run a study without an image.
    let work = common::work_of(&lab.home, "needs0");
    let err = runner::prepare_with(&lab.home, "needs0", &work, vec![Box::new(Stub(2))]);
    assert!(matches!(err, Err(RunError::Environment(_))));
}

/// Copies the whole input view to the output, records its params, writes the template's
/// measure and links `latest` to it.
const VIEW_PAYLOAD: &str = "#!/bin/sh\nset -e\n\
    cp -R \"$IGNORARIUM_IN\" \"$IGNORARIUM_OUT/in\"\n\
    printf '%s' \"$IGNORARIUM_PARAMS\" > \"$IGNORARIUM_OUT/params\"\n\
    printf '{\"ppl\": 1.5}' > \"$IGNORARIUM_OUT/values.json\"\n\
    ln -s values.json \"$IGNORARIUM_OUT/latest\"\n";

#[test]
fn a_level2_study_reads_the_sealed_view_and_records_values() {
    let Some(r) = ready() else { return };
    let lab = level2_lab();
    let store = Store::open(&lab.home.root).unwrap();
    let digest = store
        .put_reader(&mut File::open(&r.image).unwrap())
        .unwrap();
    let id = "view2";
    let params = |t| swap(t, "# params = { bits = 4 }", "params = { bits = 4 }");
    register_pinned(&lab, id, (2, 1, &digest), VIEW_PAYLOAD, params);
    let work = common::work_of(&lab.home, id);
    let plan = runner::prepare_with(&lab.home, id, &work, vec![Box::new(Level2::new())]).unwrap();
    assert_eq!(runner::run(&plan).unwrap(), RunStatus::Done);
    let dir = lab.home.study_dir(id);
    let view = tree::tree_rows(&dir.join("in")).unwrap();
    let names: Vec<&str> = view.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(names, ["data/example.txt", ignorarium::view::MANIFEST]);
    let rows = common::rows_of(&lab, id);
    let mut conditions: Vec<&str> = rows
        .iter()
        .map(|r| r.body["condition"].as_str().unwrap())
        .collect();
    conditions.sort();
    assert_eq!(conditions, ["arm-a", "exact"]);
    for rec in &rows {
        let b = &rec.body;
        assert_eq!(b["exit"], "ok", "{b}");
        assert_eq!(
            (&b["isolation"], &b["environment"]),
            (&json!(2), &json!(digest))
        );
        let agent = sha256_file(&Level2::new().agent).unwrap();
        assert_eq!(b["agent_sha256"], agent);
        assert_eq!(b["values"], json!({ "ppl": 1.5 }));
        let out = dir.join(format!("run/rows/{}/out", rec.row.unwrap()));
        assert_eq!(tree::tree_rows(&out.join("in")).unwrap(), view);
        let params = if b["condition"] == "arm-a" {
            r#"{"bits":4}"#
        } else {
            "{}"
        };
        assert_eq!(fs::read_to_string(out.join("params")).unwrap(), params);
        // The host makes no link; the row's hash covers the one the guest made.
        assert!(fs::symlink_metadata(out.join("latest")).is_err());
        let mut all = tree::tree_rows(&out).unwrap();
        all.push(tree::link_row(b"latest", b"values.json"));
        assert_eq!(b["output_hash"], tree::hash_rows(&all).unwrap());
    }
}

#[test]
fn registered_lists_levels_0_1_and_2() {
    let levels: Vec<u8> = ignorarium::backend::registered()
        .iter()
        .map(|b| b.level())
        .collect();
    assert_eq!(levels, [0, 1, 2]);
}

#[test]
fn probe_reaches_level_2_on_a_kvm_host() {
    let Some(_r) = ready() else { return };
    let lab = common::lab();
    let a = assert_cmd::Command::cargo_bin("ignorarium")
        .unwrap()
        .env("IGNORARIUM_HOME", &lab.home.root)
        .arg("probe")
        .assert()
        .code(0);
    let out = String::from_utf8_lossy(&a.get_output().stdout).into_owned();
    assert!(out.contains("max level: 2\n"), "{out}");
    assert!(
        out.contains("level 2 (cloud-hypervisor v54.0): available\n"),
        "{out}"
    );
    assert!(out.ends_with("/dev/kvm: present\n"), "{out}");
    let m = ignorarium::machine::load(&lab.home.root).unwrap();
    assert_eq!(m.max_level, 2);
}

// -- the overhead of level 2 against level 1, through the runner -------------------------------

/// Level 2 that records each row's boot time, in seconds.
struct Timed {
    inner: Level2,
    boots: Rc<RefCell<Vec<f64>>>,
}

impl Backend for Timed {
    fn level(&self) -> u8 {
        self.inner.level()
    }
    fn probe(&self) -> Probe {
        self.inner.probe()
    }
    fn launcher(&self) -> String {
        self.inner.launcher()
    }
    fn cpus(&self) -> u32 {
        self.inner.cpus()
    }
    fn run_row(&self, job: &RowJob, watch: &dyn Watch) -> io::Result<RowOutcome> {
        let (o, timings) = self.inner.run_timed(job, watch);
        if let Some(t) = timings {
            self.boots.borrow_mut().push(t.boot.as_secs_f64());
        }
        Ok(o)
    }
}

const OVERHEAD_TRIALS: usize = 11;
const OVERHEAD_ROWS: usize = 4;
/// Two-sided 95% Student t for the 10 kept trials (9 degrees of freedom).
const T95_9: f64 = 2.262;

/// The wall time of each row of study `id` but the first, in milliseconds; every row is `ok`.
fn kept_walls(lab: &common::Lab, id: &str) -> Vec<f64> {
    let mut rows = common::rows_of(lab, id);
    rows.sort_by_key(|r| r.row);
    assert_eq!(rows.len(), OVERHEAD_ROWS, "{id}");
    rows.iter()
        .skip(1)
        .map(|r| {
            assert_eq!(r.body["exit"], "ok", "{id}: {:?}", r.body);
            r.body["wall_seconds"].as_f64().unwrap() * 1000.0
        })
        .collect()
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

/// The mean of `trials` and its 95% half-width, from the spread of the trial values.
fn interval(trials: &[f64]) -> (f64, f64) {
    let m = mean(trials);
    let var = trials.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (trials.len() - 1) as f64;
    (m, T95_9 * (var / trials.len() as f64).sqrt())
}

fn report(name: &str, rows: usize, trials: &[f64]) {
    let (mean, ci95) = interval(trials);
    let trials = trials.len();
    println!(
        r#"{{"series":"{name}","unit":"ms","trials":{trials},"n":{rows},"mean":{mean:.2},"ci95":{ci95:.2}}}"#
    );
}

/// Level 1 against level 2 on one machine: 11 trials, trial 0 dropped (it builds the rootfs
/// disk and warms the cache), the order alternating, the first row of each study dropped.
/// Rows sit inside trials, so the interval comes from the 10 trial values, as in Georges et
/// al. (2007) and Laaber et al. (2019).
#[test]
#[ignore = "measurement; needs a KVM host and level 1"]
fn overhead_through_the_runner() {
    let Some(r) = ready() else { return };
    if let Some(why) = common::no_level1() {
        panic!("the overhead needs level 1: {why}");
    }
    let lab = level2_lab();
    let store = Store::open(&lab.home.root).unwrap();
    let digest = store
        .put_reader(&mut File::open(&r.image).unwrap())
        .unwrap();
    let box_exe = env!("CARGO_BIN_EXE_ignorarium");
    let (mut l1, mut l2, mut boot) = (vec![], vec![], vec![]);
    let (mut l1_rows, mut l2_rows) = (0, 0);
    for trial in 0..OVERHEAD_TRIALS {
        let boots = Rc::new(RefCell::new(Vec::new()));
        let mut walls = [Vec::new(), Vec::new()];
        // Even trials run level 1 first, odd trials level 2 first.
        let order = if trial % 2 == 0 { [1, 2] } else { [2, 1] };
        for level in order {
            let id = format!("overhead-l{level}-t{trial}");
            // Two units per cluster: 2 units x 2 conditions = 4 rows.
            register_pinned(&lab, &id, (level, 2, &digest), common::PAYLOAD, |t| t);
            let backend: Box<dyn Backend> = if level == 1 {
                Box::new(Level1::with(Level1::new().bwrap, box_exe.into()))
            } else {
                Box::new(Timed {
                    inner: Level2::new(),
                    boots: boots.clone(),
                })
            };
            let work = common::work_of(&lab.home, &id);
            let plan = runner::prepare_with(&lab.home, &id, &work, vec![backend]).unwrap();
            assert_eq!(runner::run(&plan).unwrap(), RunStatus::Done, "{id}");
            walls[level as usize - 1] = kept_walls(&lab, &id);
        }
        if trial == 0 {
            continue;
        }
        let boots = boots.borrow();
        assert_eq!(boots.len(), OVERHEAD_ROWS, "one boot per level 2 row");
        l1_rows += walls[0].len();
        l2_rows += walls[1].len();
        l1.push(mean(&walls[0]));
        l2.push(mean(&walls[1]));
        boot.push(mean(&boots[1..]) * 1000.0);
    }
    assert_eq!((l1_rows, l2_rows), (30, 30));
    let diff: Vec<f64> = l2.iter().zip(&l1).map(|(b, a)| b - a).collect();
    for series in [&l1, &l2, &boot] {
        assert!(
            series.iter().all(|x| x.is_finite() && *x > 0.0),
            "{series:?}"
        );
    }
    report("l1_row", l1_rows, &l1);
    report("l2_row", l2_rows, &l2);
    report("l2_boot", l2_rows, &boot);
    report("l2_minus_l1", l2_rows, &diff);
}
