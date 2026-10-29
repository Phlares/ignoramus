mod common;
use common::{file, pack, packed, write_exec};
use ignorarium_guest::host::{HostJob, Vm, VmError, VmOutcome, run_row, vmm_args};
use ignorarium_guest::proto::{
    End, Job, MAX_FRAME, Report, SelfTest, ToHost, read_frame, write_frame,
};
use ignorarium_guest::tree::tree_hash;
use std::fs::{self, Permissions};
use std::io::{ErrorKind, Read, Seek, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const SLEEPER: &str = "exec sleep 60\n";

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// What the fake guest knows about the row.
struct Ctx {
    work: PathBuf,
    src: PathBuf,
    job: Job,
}

struct Rig {
    root: tempfile::TempDir,
    src: PathBuf,
    work: PathBuf,
    out: PathBuf,
    vm: Vm,
    job: HostJob,
}

impl Rig {
    fn new(vmm_script: &str) -> Rig {
        let root = tempfile::tempdir().unwrap();
        let p = root.path();
        let (src, work, out, input) = (p.join("src"), p.join("work"), p.join("out"), p.join("in"));
        for d in [&src, &work, &out, &input] {
            fs::create_dir(d).unwrap();
        }
        fs::write(src.join("a.txt"), "hello\n").unwrap();
        fs::create_dir(src.join("sub")).unwrap();
        fs::write(src.join("sub/b"), vec![7u8; 5000]).unwrap();
        fs::write(input.join("data"), "input\n").unwrap();
        let vmm = p.join("fake-vmm");
        let script = format!(
            "#!/bin/sh
{vmm_script}"
        )
        .replace("$PIDFILE", p.join("vmm.pid").to_str().unwrap());
        write_exec(&vmm, &script);
        let vm = Vm {
            vmm,
            kernel: "kernel".into(),
            initramfs: "initramfs".into(),
            rootfs: "rootfs.img".into(),
            cpus: 2,
            mem_mib: 256,
            out_mib: 8,
            boot_timeout: Duration::from_secs(10),
            exit_timeout: ms(50),
            report_slack: Duration::from_secs(5),
            serial_log: None,
        };
        let job = HostJob {
            argv: vec!["/bin/prog".into(), "--x".into()],
            env: vec![("K".into(), "V".into())],
            input_dir: input,
            output_dir: out.clone(),
            seed: [0xab; 32],
            wall_timeout: Duration::from_secs(10),
        };
        Rig {
            root,
            src,
            work,
            out,
            vm,
            job,
        }
    }

    /// A fake VMM that records its pid, then sleeps.
    fn sleeper() -> Rig {
        Rig::new(&format!("echo $$ > $PIDFILE\n{SLEEPER}"))
    }

    fn run(&self) -> Result<VmOutcome, VmError> {
        run_row(&self.vm, &self.job, &self.work)
    }

    fn pid_gone(&self) {
        let pid = wait_pid(self.root.path());
        assert!(
            !Path::new("/proc").join(&pid).exists(),
            "VMM {pid} still runs"
        );
    }

    fn src_hash(&self) -> String {
        tree_hash(&self.src).unwrap()
    }

    /// Plays the guest: connects to the listener path, reads nothing, then runs `f`.
    fn guest(&self, f: impl FnOnce(&mut UnixStream, &mut Ctx) + Send + 'static) -> JoinHandle<()> {
        let sock = self.work.join("vsock.sock_7201");
        let root = self.root.path().to_path_buf();
        let (work, src) = (self.work.clone(), self.src.clone());
        let blank = Job {
            seed_hex: String::new(),
            unix_nanos: 0,
            argv: vec![],
            env: vec![],
            timeout_ms: 0,
        };
        thread::spawn(move || {
            // The VMM is running (its pid is recorded) before the guest connects, so the
            // kill checks are never vacuous.
            wait_pid(&root);
            let t = Instant::now();
            let mut s = loop {
                match UnixStream::connect(&sock) {
                    Ok(s) => break s,
                    Err(e) if t.elapsed() > Duration::from_secs(5) => panic!("connect: {e}"),
                    Err(_) => thread::sleep(ms(2)),
                }
            };
            f(
                &mut s,
                &mut Ctx {
                    work,
                    src,
                    job: blank,
                },
            );
        })
    }
}

/// The fake VMM's pid, once its script has recorded it.
fn wait_pid(root: &Path) -> String {
    let t = Instant::now();
    loop {
        let pid = fs::read_to_string(root.join("vmm.pid")).unwrap_or_default();
        if !pid.trim().is_empty() {
            return pid.trim().to_string();
        }
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "the VMM never started"
        );
        thread::sleep(ms(1));
    }
}

/// Sends one byte of a never-finished frame every `every`, until the host hangs up.
fn trickle(s: &mut UnixStream, every: Duration) {
    let mut bytes = vec![0, 0, 0xff, 0xff];
    bytes.extend([b'x'; 60]);
    for b in bytes {
        if s.write_all(&[b]).is_err() {
            return;
        }
        thread::sleep(every);
    }
}

fn hello(s: &mut UnixStream) {
    let agent = "fake".to_string();
    write_frame(s, &ToHost::Hello { agent }).unwrap();
}

fn read_job(s: &mut UnixStream, ctx: &mut Ctx) {
    ctx.job = read_frame(s).unwrap();
}

fn report(hash: &str, end: Option<End>) -> ToHost {
    ToHost::Report(Report {
        selftest: SelfTest {
            only_loopback: true,
            route_blocked: true,
            host_path_blocked: true,
        },
        clock_unix_nanos: 1,
        end,
        payload_wall_ns: 5_000_000,
        cpu_seconds: 0.5,
        output_hash: hash.into(),
    })
}

fn put_out(work: &Path, bytes: &[u8]) {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .open(work.join("out.img"))
        .unwrap();
    f.rewind().unwrap();
    f.write_all(bytes).unwrap();
    f.sync_all().unwrap();
}

fn failed(reason: &str) -> ToHost {
    ToHost::Failed {
        reason: reason.into(),
    }
}

fn good_guest(rig: &Rig) -> JoinHandle<()> {
    rig.guest(|s, c| {
        hello(s);
        read_job(s, c);
        put_out(&c.work, &packed(&c.src));
        let h = tree_hash(&c.src).unwrap();
        write_frame(s, &report(&h, Some(End::Code(0)))).unwrap();
    })
}

fn args_of(vm: &Vm, work: &Path) -> Vec<String> {
    vmm_args(vm, work)
        .into_iter()
        .map(|a| a.into_string().unwrap())
        .collect()
}

fn after<'a>(args: &'a [String], flag: &str) -> &'a str {
    let i = args.iter().position(|a| a == flag).unwrap();
    &args[i + 1]
}

#[test]
fn vmm_args_have_no_network_and_read_only_inputs() {
    let rig = Rig::sleeper();
    let a = args_of(&rig.vm, Path::new("/w"));
    assert!(!a.iter().any(|x| x == "--net" || x == "--api-socket"));
    let i = a.iter().position(|x| x == "--disk").unwrap();
    assert_eq!(
        &a[i + 1..i + 4],
        [
            "path=rootfs.img,readonly=on,image_type=raw,serial=ign-rootfs",
            "path=/w/in.img,readonly=on,image_type=raw,serial=ign-in",
            "path=/w/out.img,image_type=raw,serial=ign-out",
        ]
    );
    assert_eq!(after(&a, "--vsock"), "cid=3,socket=/w/vsock.sock");
    assert_eq!(after(&a, "--serial"), "off");
    assert_eq!(after(&a, "--console"), "off");
    assert_eq!(after(&a, "--cmdline"), "quiet");
    assert_eq!(after(&a, "--kernel"), "kernel");
    assert_eq!(after(&a, "--initramfs"), "initramfs");
    assert_eq!(after(&a, "--cpus"), "boot=2");
    assert_eq!(after(&a, "--memory"), "size=256M");
}

#[test]
fn vmm_args_log_the_serial_console_on_request() {
    let mut rig = Rig::sleeper();
    rig.vm.serial_log = Some("/logs/serial.txt".into());
    let a = args_of(&rig.vm, Path::new("/w"));
    assert_eq!(after(&a, "--serial"), "file=/logs/serial.txt");
    assert!(after(&a, "--cmdline").contains("console=ttyS0"));
}

#[test]
fn a_row_round_trip_through_a_fake_guest() {
    let rig = Rig::sleeper();
    let (seed, argv, env) = (rig.job.seed, rig.job.argv.clone(), rig.job.env.clone());
    let wall = rig.job.wall_timeout.as_millis() as u64;
    let h = rig.guest(move |s, c| {
        hello(s);
        read_job(s, c);
        assert_eq!(c.job.seed_hex, hex::encode(seed));
        assert_eq!(c.job.argv, argv);
        assert_eq!(c.job.env, env);
        assert_eq!(c.job.timeout_ms, wall);
        assert!(c.job.unix_nanos > 1_000_000_000_000_000_000);
        put_out(&c.work, &packed(&c.src));
        let hash = tree_hash(&c.src).unwrap();
        write_frame(s, &report(&hash, Some(End::Code(0)))).unwrap();
    });
    let res = rig.run();
    h.join().unwrap();
    let o = res.unwrap();
    assert_eq!(o.output_hash, rig.src_hash());
    assert_eq!(tree_hash(&rig.out).unwrap(), rig.src_hash());
    assert_eq!(o.report.end, Some(End::Code(0)));
    assert!(o.timings.boot <= o.timings.total);
    assert!(o.timings.prepare <= o.timings.total);
    assert_eq!(o.timings.payload, Duration::from_nanos(5_000_000));
    rig.pid_gone();
}

#[test]
fn the_listener_is_gone_after_the_guest_connects() {
    let rig = Rig::sleeper();
    let h = rig.guest(|s, c| {
        hello(s);
        read_job(s, c);
        let path = c.work.join("vsock.sock_7201");
        assert!(UnixStream::connect(&path).is_err());
        assert!(!path.exists());
        write_frame(s, &failed("done")).unwrap();
    });
    let res = rig.run();
    h.join().unwrap();
    assert!(res.is_err());
}

#[test]
fn a_hash_mismatch_is_an_error_with_the_guest_hash_escaped() {
    let rig = Rig::sleeper();
    let h = rig.guest(move |s, c| {
        hello(s);
        read_job(s, c);
        put_out(&c.work, &packed(&c.src));
        let bells = "\u{7}".repeat(300);
        write_frame(s, &report(&bells, Some(End::Code(0)))).unwrap();
    });
    let res = rig.run();
    h.join().unwrap();
    match &res {
        Err(e @ VmError::Mismatch { guest, host }) => {
            assert_eq!(*guest, format!("{:?}", "\u{7}".repeat(200) + "..."));
            assert_eq!(*host, rig.src_hash());
            let msg = e.to_string();
            assert!(msg.contains(&format!("guest said {guest},")), "{msg}");
        }
        other => panic!("{:?}", other),
    }
}

#[test]
fn a_hostile_pack_is_a_guest_error() {
    let rig = Rig::sleeper();
    let h = rig.guest(|s, c| {
        hello(s);
        read_job(s, c);
        put_out(&c.work, &pack(&[file(b"../x", b"x")]));
        write_frame(s, &report("00", Some(End::Code(0)))).unwrap();
    });
    let res = rig.run();
    h.join().unwrap();
    match res {
        Err(VmError::Guest(m)) => {
            // The pack error is quoted and escaped like other guest text.
            assert!(m.contains("bad output pack: \""), "{m}");
            assert!(m.contains("vmm.log tail"), "{m}");
        }
        other => panic!("{:?}", other),
    }
    assert!(!rig.root.path().join("x").exists());
    assert_eq!(fs::read_dir(&rig.out).unwrap().count(), 0);
}

#[test]
fn a_failed_guest_is_an_error() {
    let rig = Rig::sleeper();
    let h = rig.guest(|s, c| {
        hello(s);
        read_job(s, c);
        write_frame(s, &failed("no disk")).unwrap();
    });
    let res = rig.run();
    h.join().unwrap();
    match res {
        Err(VmError::Guest(m)) => assert!(m.contains("no disk"), "{m}"),
        other => panic!("{:?}", other),
    }
    rig.pid_gone();
}

#[test]
fn a_failed_selftest_returns_the_report_without_outputs() {
    let rig = Rig::sleeper();
    let h = rig.guest(|s, c| {
        hello(s);
        read_job(s, c);
        write_frame(s, &report("", None)).unwrap();
    });
    let res = rig.run();
    h.join().unwrap();
    let o = res.unwrap();
    assert_eq!(o.output_hash, "");
    assert_eq!(o.report.end, None);
    assert_eq!(fs::read_dir(&rig.out).unwrap().count(), 0);
}

#[test]
fn boot_fails_fast_when_the_vmm_exits() {
    let rig = Rig::new("echo boom >&2\nexit 3\n");
    let t = Instant::now();
    let res = rig.run();
    assert!(t.elapsed() < Duration::from_secs(5));
    match res {
        Err(VmError::Boot(m)) => assert!(m.contains("exit status: 3") && m.contains("boom"), "{m}"),
        other => panic!("{:?}", other),
    }
}

#[test]
fn boot_times_out_and_kills_the_vmm() {
    let mut rig = Rig::sleeper();
    rig.vm.boot_timeout = ms(300);
    match rig.run() {
        Err(VmError::Boot(m)) => assert!(m.contains("timed out"), "{m}"),
        other => panic!("{:?}", other),
    }
    rig.pid_gone();
}

#[test]
fn a_silent_guest_times_out() {
    let mut rig = Rig::sleeper();
    rig.job.wall_timeout = ms(100);
    rig.vm.report_slack = ms(100);
    let h = rig.guest(|s, c| {
        hello(s);
        read_job(s, c);
        thread::sleep(ms(600));
    });
    let t = Instant::now();
    let res = rig.run();
    assert!(t.elapsed() < Duration::from_secs(2));
    h.join().unwrap();
    assert!(matches!(res, Err(VmError::Timeout(_))), "{:?}", res);
    rig.pid_gone();
}

#[test]
fn a_closed_channel_is_a_guest_error() {
    let rig = Rig::sleeper();
    let h = rig.guest(|s, c| {
        hello(s);
        read_job(s, c);
    });
    let res = rig.run();
    h.join().unwrap();
    match res {
        Err(VmError::Guest(m)) => assert!(m.contains("closed"), "{m}"),
        other => panic!("{:?}", other),
    }
}

#[test]
fn the_vmm_is_killed_when_it_outlives_the_report() {
    let mut rig = Rig::sleeper();
    rig.vm.exit_timeout = ms(100);
    // A big output makes read-back slow, so a VMM still alive during it shows up.
    fs::write(rig.src.join("big"), vec![1u8; 6 << 20]).unwrap();
    let root = rig.root.path().to_path_buf();
    let watcher = thread::spawn(move || {
        let pid = wait_pid(&root);
        while Path::new("/proc").join(&pid).exists() {
            thread::sleep(ms(1));
        }
        Instant::now()
    });
    let h = good_guest(&rig);
    let res = rig.run();
    let returned = Instant::now();
    h.join().unwrap();
    let o = res.unwrap();
    let died = watcher.join().unwrap();
    assert!(o.timings.stop >= ms(100));
    assert!(o.timings.stop < Duration::from_secs(1));
    assert!(o.timings.extract >= ms(5), "read-back too fast to tell");
    // The VMM was gone well before read-back ended, not only when run_row returned.
    let gap = returned.saturating_duration_since(died);
    assert!(
        gap >= o.timings.extract / 2,
        "VMM died {gap:?} before return, read-back took {:?}",
        o.timings.extract
    );
    rig.pid_gone();
}

#[test]
fn work_dir_limits_are_checked_before_boot() {
    let rig = Rig::new(&format!("touch $PIDFILE\n{SLEEPER}"));
    let mut long = rig.root.path().to_path_buf();
    while long.as_os_str().len() < 120 {
        long.push("d".repeat(60));
    }
    fs::create_dir_all(&long).unwrap();
    let res = run_row(&rig.vm, &rig.job, &long);
    assert!(matches!(res, Err(VmError::Io(_))), "{:?}", res);
    thread::sleep(ms(100));
    assert!(!rig.root.path().join("vmm.pid").exists());
    assert_eq!(fs::read_dir(&long).unwrap().count(), 0);
}

#[test]
fn a_non_empty_work_dir_is_refused() {
    let rig = Rig::sleeper();
    fs::write(rig.work.join("stale"), "x").unwrap();
    let res = rig.run();
    assert!(matches!(res, Err(VmError::Io(_))), "{:?}", res);
}

#[test]
fn a_trickled_hello_cannot_stretch_the_boot_deadline() {
    let mut rig = Rig::sleeper();
    rig.vm.boot_timeout = ms(300);
    let h = rig.guest(|s, _| trickle(s, ms(100)));
    let t = Instant::now();
    let res = rig.run();
    assert!(t.elapsed() < ms(900), "{:?}", t.elapsed());
    h.join().unwrap();
    match res {
        Err(VmError::Boot(m)) => assert!(m.contains("vmm.log tail"), "{m}"),
        other => panic!("{:?}", other),
    }
    rig.pid_gone();
}

#[test]
fn a_trickled_report_cannot_stretch_the_report_deadline() {
    let mut rig = Rig::sleeper();
    rig.job.wall_timeout = ms(150);
    rig.vm.report_slack = ms(150);
    let h = rig.guest(|s, c| {
        hello(s);
        read_job(s, c);
        trickle(s, ms(100));
    });
    let t = Instant::now();
    let res = rig.run();
    assert!(t.elapsed() < ms(900), "{:?}", t.elapsed());
    h.join().unwrap();
    match res {
        Err(VmError::Timeout(m)) => assert!(m.contains("vmm.log tail"), "{m}"),
        other => panic!("{:?}", other),
    }
    rig.pid_gone();
}

#[test]
fn a_guest_that_never_reads_the_job_times_out() {
    let mut rig = Rig::sleeper();
    rig.job.argv = vec!["a".repeat(900_000)];
    rig.job.wall_timeout = ms(100);
    rig.vm.report_slack = ms(100);
    let h = rig.guest(|s, _| {
        hello(s);
        thread::sleep(ms(1500));
    });
    let t = Instant::now();
    let res = rig.run();
    assert!(t.elapsed() < ms(1000), "{:?}", t.elapsed());
    h.join().unwrap();
    assert!(matches!(res, Err(VmError::Timeout(_))), "{:?}", res);
}

#[test]
fn guest_text_in_errors_is_capped_and_escaped() {
    let rig = Rig::sleeper();
    let h = rig.guest(|s, c| {
        hello(s);
        read_job(s, c);
        write_frame(s, &failed(&"\u{7}".repeat(150_000))).unwrap();
    });
    let res = rig.run();
    h.join().unwrap();
    match res {
        Err(VmError::Guest(m)) => {
            assert!(m.len() < 3000, "{} bytes", m.len());
            assert!(!m.contains('\u{7}'));
        }
        other => panic!("{:?}", other),
    }
}

/// Returns the refusal's text.
fn refused_before_boot(rig: &Rig, work: &Path) -> String {
    let res = run_row(&rig.vm, &rig.job, work);
    let Err(VmError::Io(e)) = res else {
        panic!("{res:?}")
    };
    assert_eq!(e.kind(), ErrorKind::InvalidInput, "{e}");
    thread::sleep(ms(100));
    assert!(!rig.root.path().join("vmm.pid").exists());
    assert_eq!(fs::read_dir(work).unwrap().count(), 0);
    e.to_string()
}

#[test]
fn a_job_over_the_frame_limit_is_refused_before_boot() {
    let mut rig = Rig::new(&format!("touch $PIDFILE\n{SLEEPER}"));
    rig.job.argv = vec!["a".repeat(MAX_FRAME)];
    let msg = refused_before_boot(&rig, &rig.work);
    assert!(msg.contains("frame limit"), "{msg}");
}

#[test]
fn a_non_empty_output_dir_is_refused_before_boot() {
    let rig = Rig::new(&format!("touch $PIDFILE\n{SLEEPER}"));
    fs::write(rig.out.join("old"), "x").unwrap();
    refused_before_boot(&rig, &rig.work);
}

#[test]
fn a_comma_in_a_path_is_refused_before_boot() {
    let mut rig = Rig::new(&format!("touch $PIDFILE\n{SLEEPER}"));
    let comma = rig.root.path().join("w,x");
    fs::create_dir(&comma).unwrap();
    refused_before_boot(&rig, &comma);
    let set: [fn(&mut Vm); 4] = [
        |v| v.kernel = "k,x".into(),
        |v| v.initramfs = "i,x".into(),
        |v| v.rootfs = "r,x".into(),
        |v| v.serial_log = Some("s,x".into()),
    ];
    for f in set {
        let saved = (
            rig.vm.kernel.clone(),
            rig.vm.initramfs.clone(),
            rig.vm.rootfs.clone(),
        );
        f(&mut rig.vm);
        refused_before_boot(&rig, &rig.work);
        (rig.vm.kernel, rig.vm.initramfs, rig.vm.rootfs) = saved;
        rig.vm.serial_log = None;
    }
}

#[test]
fn a_zero_report_budget_is_a_timeout() {
    let mut rig = Rig::sleeper();
    rig.job.wall_timeout = Duration::ZERO;
    rig.vm.report_slack = Duration::ZERO;
    let h = rig.guest(|s, _| {
        hello(s);
        thread::sleep(ms(300));
    });
    let res = rig.run();
    h.join().unwrap();
    assert!(matches!(res, Err(VmError::Timeout(_))), "{:?}", res);
}

#[test]
fn a_slow_drain_cannot_stretch_the_job_write() {
    let mut rig = Rig::sleeper();
    rig.job.argv = vec!["a".repeat(900_000)];
    rig.job.wall_timeout = ms(100);
    rig.vm.report_slack = ms(100);
    let h = rig.guest(|s, _| {
        hello(s);
        // Takes a little often, so no single wait for buffer space runs out.
        let mut buf = [0u8; 8192];
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(4) && matches!(s.read(&mut buf), Ok(n) if n > 0) {
            thread::sleep(ms(20));
        }
    });
    let t = Instant::now();
    let res = rig.run();
    assert!(t.elapsed() < ms(1000), "{:?}", t.elapsed());
    h.join().unwrap();
    assert!(matches!(res, Err(VmError::Timeout(_))), "{:?}", res);
}

#[test]
fn channel_errors_hold_no_raw_guest_bytes() {
    let rig = Rig::sleeper();
    let h = rig.guest(|s, _| {
        // Valid JSON text, but the tag names no variant, so the parser quotes it back.
        let body = format!("{{\"type\":\"{}\"}}", r"\u0007".repeat(5000));
        s.write_all(&(body.len() as u32).to_be_bytes()).unwrap();
        s.write_all(body.as_bytes()).unwrap();
    });
    let res = rig.run();
    h.join().unwrap();
    match res {
        Err(VmError::Guest(m)) => {
            assert!(m.contains("control channel failed"), "{m}");
            assert!(!m.contains('\u{7}'), "raw control byte in {m:?}");
            assert!(m.len() < 3000, "{} bytes", m.len());
        }
        other => panic!("{:?}", other),
    }
}

#[test]
fn output_disks_of_zero_or_over_a_quarter_of_the_memory_are_refused_before_boot() {
    let mut rig = Rig::new(&format!("touch $PIDFILE\n{SLEEPER}"));
    // A tmpfs of size 0 has no limit; /out and /tmp live in guest memory, each up to the
    // disk size.
    for (out_mib, mem_mib) in [(0, 256), (65, 256)] {
        (rig.vm.out_mib, rig.vm.mem_mib) = (out_mib, mem_mib);
        let msg = refused_before_boot(&rig, &rig.work);
        assert!(msg.contains("output disk"), "{msg}");
    }
}

/// The checks before boot pass when the fake VMM starts and the boot then times out.
fn boots(rig: &mut Rig) {
    rig.vm.boot_timeout = ms(200);
    let res = rig.run();
    assert!(matches!(res, Err(VmError::Boot(_))), "{res:?}");
    rig.pid_gone();
}

#[test]
fn an_output_disk_of_a_quarter_of_the_memory_boots() {
    let mut rig = Rig::sleeper();
    (rig.vm.out_mib, rig.vm.mem_mib) = (64, 256);
    boots(&mut rig);
}

#[test]
fn inputs_the_payload_cannot_read_are_refused_before_boot() {
    let rig = Rig::new(&format!("touch $PIDFILE\n{SLEEPER}"));
    let input = &rig.job.input_dir;
    let (secret, sub) = (input.join("secret"), input.join("sub"));
    fs::write(&secret, "x").unwrap();
    fs::create_dir(&sub).unwrap();
    let mode = |p: &Path, m: u32| fs::set_permissions(p, Permissions::from_mode(m)).unwrap();
    // A file without other-read, a directory without other-read, one without other-execute.
    let cases = [
        (&secret, 0o640, 0o644),
        (&sub, 0o711, 0o755),
        (&sub, 0o754, 0o755),
    ];
    for (path, bad, good) in cases {
        mode(path, bad);
        let msg = refused_before_boot(&rig, &rig.work);
        assert!(msg.contains(path.to_str().unwrap()), "{msg}");
        mode(path, good);
    }
    fs::write(sub.join("deep"), "x").unwrap();
    mode(&sub.join("deep"), 0o600);
    let msg = refused_before_boot(&rig, &rig.work);
    assert!(msg.contains("deep"), "{msg}");
}

#[test]
fn readable_inputs_and_links_boot() {
    let mut rig = Rig::sleeper();
    let input = rig.job.input_dir.clone();
    fs::create_dir(input.join("sub")).unwrap();
    fs::write(input.join("sub/f"), "x").unwrap();
    // A link is not followed, so its target need not exist.
    symlink("missing", input.join("link")).unwrap();
    boots(&mut rig);
}
