//! Level 2: each row in a fresh Cloud Hypervisor virtual machine. A test that boots one starts
//! with `ready()`, which prints `skip level2: <reason>` and returns `None` where this host
//! cannot run level 2.
mod common;
use common::{in_child, is_child, write_exec};
use ignorarium::backend::level2::{Level2, guest_job, outcome_of, rootfs_disk, selftest_of};
use ignorarium::backend::selftest::SelfTest;
use ignorarium::backend::{Backend, Exit, Probe, RowJob, TIMEOUT_CODE, Watch, output_hash, rootfs};
use ignorarium::store::Store;
use ignorarium_guest::host::{Timings, VmError, VmOutcome};
use ignorarium_guest::image::{KERNEL, VMM, sha256_file};
use ignorarium_guest::proto::{self, End, Report};
use rustix::process::{Pid, getpgid, test_kill_process_group};
use std::cell::{Cell, RefCell};
use std::ffi::OsStr;
use std::fs::{self, File, Permissions};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
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
    #[expect(dead_code, reason = "for tests that register a study from the image")]
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
    rootfs_disk(&root).unwrap();
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

/// The probe's faults, without the one for `mkfs.ext4`, which depends on the host.
fn faults(b: &Level2) -> Vec<String> {
    let Probe::Unavailable(why) = b.probe() else {
        panic!("probed available")
    };
    why.split("; ")
        .filter(|f| !f.starts_with("mkfs.ext4 cannot run: "))
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
         mkfs.ext4 cannot run: No such file or directory (os error 2)",
        kvm.display()
    );
    assert_eq!(b.probe(), Probe::Unavailable(want));
}

#[test]
fn probe_checks_both_pins() {
    let d = TempDir::new().unwrap();
    let (x, agent) = (d.path().join("x"), d.path().join("agent"));
    fs::write(&x, "x").unwrap();
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
        timings: Timings::default(),
    })
}

/// The result, then the exit, CPU seconds, whether the CPU is unknown, the self-test and
/// the output hash it maps to.
type Case = (
    Result<VmOutcome, VmError>,
    Exit,
    f64,
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
            ok,
            "h",
        ),
        (
            ran(Some(End::Code(3)), passing()),
            Exit::Payload(3),
            0.75,
            false,
            ok,
            "h",
        ),
        (
            ran(Some(End::Signal(9)), passing()),
            Exit::Payload(137),
            0.75,
            false,
            ok,
            "h",
        ),
        (
            ran(Some(End::Timeout), passing()),
            Exit::Payload(TIMEOUT_CODE),
            0.75,
            false,
            ok,
            "h",
        ),
        (
            ran(None, no_route),
            infra("guest self-test failed: connect was not blocked"),
            0.75,
            false,
            Some(mapped(false, true)),
            "",
        ),
        (
            ran(None, passing()),
            infra("the guest ran no payload"),
            0.75,
            false,
            ok,
            "",
        ),
        (
            refused(End::Code(0)),
            Exit::Void("cannot pack output: too big".into()),
            0.75,
            false,
            ok,
            "",
        ),
        (refused(End::Code(3)), Exit::Payload(3), 0.75, false, ok, ""),
        (
            Err(VmError::Boot("b".into())),
            infra("boot failed: b"),
            0.0,
            false,
            None,
            "",
        ),
        (Err(VmError::Stopped), Exit::Stopped, 0.0, false, None, ""),
        (
            Err(VmError::Timeout("t".into())),
            infra("timed out: t"),
            0.0,
            true,
            None,
            "",
        ),
        (
            Err(VmError::Guest("g".into())),
            infra("guest error: g"),
            0.0,
            true,
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
            None,
            "",
        ),
        (
            io(io::ErrorKind::InvalidInput),
            infra("host I/O error: x"),
            0.0,
            false,
            None,
            "",
        ),
        (
            io(io::ErrorKind::Other),
            infra("host I/O error: x"),
            0.0,
            true,
            None,
            "",
        ),
    ];
    for (i, (res, exit, cpu, unknown, selftest, hash)) in table.into_iter().enumerate() {
        let o = outcome_of(res, 2.5);
        assert_eq!(o.exit, exit, "case {i}");
        assert_eq!(o.cpu_seconds, cpu, "case {i}");
        assert_eq!(
            o.cpu_unknown.as_deref(),
            unknown.then_some(NO_REPORT),
            "case {i}"
        );
        assert_eq!(o.selftest, selftest, "case {i}");
        assert_eq!(o.output_hash, hash, "case {i}");
        assert_eq!(o.wall_seconds, 2.5, "case {i}");
    }
}

#[test]
fn rootfs_disk_is_built_once() {
    let d = TempDir::new().unwrap();
    let root = d.path().join("root");
    fs::create_dir_all(root.join("usr/bin")).unwrap();
    fs::write(root.join("usr/bin/tool"), "x").unwrap();
    let disk = rootfs_disk(&root).unwrap();
    assert_eq!(disk, d.path().join("root.ext4"));
    // The ext4 superblock starts at byte 1024; its magic 0xEF53 sits 56 bytes in.
    assert_eq!(fs::read(&disk).unwrap()[1080..1082], [0x53, 0xef]);
    let built = fs::metadata(&disk).unwrap().modified().unwrap();
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(rootfs_disk(&root).unwrap(), disk);
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
    // A failed build leaves nothing beside the root: before mkfs runs, or in it.
    assert!(rootfs_disk(&d.path().join("absent")).is_err());
    assert_eq!(names(), ["root", "root.ext4"]);
    let bad = d.path().join("bad");
    fs::create_dir(&bad).unwrap();
    fs::write(bad.join("secret"), "x").unwrap();
    fs::set_permissions(bad.join("secret"), Permissions::from_mode(0o000)).unwrap();
    if File::open(bad.join("secret")).is_ok() {
        println!("skipped the mkfs failure: this user reads a mode-000 file");
        return;
    }
    assert!(rootfs_disk(&bad).is_err());
    assert_eq!(names(), ["bad", "root", "root.ext4"]);
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
    let (o, timings) = b.run_timed(&job, &watch).unwrap();
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
         > \"$IGNORARIUM_OUT/paths\"",
        LONG,
    );
    let o = r.level2.run_row(&job, &|| true).unwrap();
    assert_eq!(o.exit, Exit::Ok, "{o:?}");
    assert_eq!(read(&job, "paths"), "/payload/run /in /out --condition a");
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
/// socket then, so the payload is running by the stop.
#[derive(Default)]
struct StopOnceRunning {
    pid: Cell<Option<u32>>,
    listen: RefCell<Option<PathBuf>>,
    stop_at: Cell<Option<Instant>>,
}

impl Watch for StopOnceRunning {
    fn tick(&self) -> bool {
        if let Some(at) = self.stop_at.get() {
            return Instant::now() < at;
        }
        if self.listen.borrow().as_ref().is_some_and(|p| !p.exists()) {
            self.stop_at
                .set(Some(Instant::now() + Duration::from_millis(500)));
        }
        true
    }

    fn started(&self, pid: u32) {
        self.pid.set(Some(pid));
        let cmdline = fs::read(format!("/proc/{pid}/cmdline")).unwrap();
        let args: Vec<&[u8]> = cmdline.split(|&b| b == 0).collect();
        let at = args.iter().position(|a| *a == b"--vsock").unwrap();
        let vsock = String::from_utf8(args[at + 1].to_vec()).unwrap();
        let socket = vsock.split_once("socket=").unwrap().1;
        *self.listen.borrow_mut() = Some(PathBuf::from(format!("{socket}_7201")));
    }
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
