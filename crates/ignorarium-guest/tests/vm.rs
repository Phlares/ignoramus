//! Rows in fresh Cloud Hypervisor VMs. They are ignored by default: they need `/dev/kvm`, an
//! agent built static (see `STATIC_BUILD`) and three files named by environment variables:
//! `IGNORARIUM_VM_VMM` (the VMM binary), `IGNORARIUM_VM_KERNEL` (its guest kernel) and
//! `IGNORARIUM_VM_ROOTFS` (an ext4 image of the environment: a merged-usr root with `sh`,
//! coreutils, `setsid` and Python with numpy).
use ignorarium_guest::agent::steps::now_nanos;
use ignorarium_guest::host::{HostJob, REPORT_SLACK, Vm, VmError, VmOutcome, run_row};
use ignorarium_guest::image::initramfs;
use ignorarium_guest::proto::End;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const STATIC_BUILD: &str = "RUSTFLAGS='-C target-feature=+crt-static' cargo test \
    --target x86_64-unknown-linux-gnu -p ignorarium-guest -- --include-ignored";
const SEED: [u8; 32] = *b"a fixed seed for the vm tests 32";

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn var(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("set {name}; see the top of tests/vm.rs"))
}

/// True when the ELF file names a program interpreter, so it needs a dynamic loader.
fn has_interp(elf: &[u8]) -> bool {
    const PT_INTERP: u32 = 3;
    assert!(elf.starts_with(b"\x7fELF\x02\x01"), "not a 64-bit LE ELF");
    let le = |at: usize, n: usize| {
        elf[at..at + n]
            .iter()
            .rev()
            .fold(0usize, |v, &b| v << 8 | b as usize)
    };
    let (phoff, size, count) = (le(0x20, 8), le(0x36, 2), le(0x38, 2));
    (0..count).any(|i| le(phoff + i * size, 4) == PT_INTERP as usize)
}

struct Row {
    dir: tempfile::TempDir,
    vm: Vm,
    job: HostJob,
}

impl Row {
    /// A row that runs `script` with `/bin/sh -c`. The input dir holds `data.txt`.
    fn sh(script: &str) -> Row {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        for d in ["in", "out", "work"] {
            fs::create_dir(p.join(d)).unwrap();
        }
        fs::write(p.join("in/data.txt"), "input bytes\n").unwrap();
        let agent = Path::new(env!("CARGO_BIN_EXE_ignorarium-guest"));
        let bytes = fs::read(agent).unwrap();
        assert!(
            !has_interp(&bytes),
            "{} is linked dynamically, so it cannot run as /init; build it static: {STATIC_BUILD}",
            agent.display()
        );
        fs::write(p.join("initramfs.cpio"), initramfs(&bytes)).unwrap();
        let vm = Vm {
            vmm: var("IGNORARIUM_VM_VMM"),
            kernel: var("IGNORARIUM_VM_KERNEL"),
            initramfs: p.join("initramfs.cpio"),
            rootfs: var("IGNORARIUM_VM_ROOTFS"),
            cpus: 1,
            mem_mib: 512,
            out_mib: 16,
            boot_timeout: secs(30),
            exit_timeout: secs(10),
            report_slack: REPORT_SLACK,
            serial_log: Some(p.join("serial.log")),
        };
        let job = HostJob {
            argv: ["/bin/sh", "-c", script].map(String::from).to_vec(),
            env: vec![("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into())],
            input_dir: p.join("in"),
            output_dir: p.join("out"),
            seed: SEED,
            wall_timeout: secs(60),
        };
        Row { dir, vm, job }
    }

    fn try_run(&self) -> Result<VmOutcome, VmError> {
        let res = run_row(&self.vm, &self.job, &self.dir.path().join("work"));
        if let Ok(o) = &res {
            eprintln!("{:?}", o.timings);
        }
        res
    }

    /// Runs the row. On an error, panics with it and the guest console.
    fn run(&self) -> VmOutcome {
        self.try_run().unwrap_or_else(|e| {
            let console = fs::read_to_string(self.dir.path().join("serial.log"));
            panic!("{e}\nguest console:\n{}", console.unwrap_or_default())
        })
    }

    fn out(&self, name: &str) -> String {
        fs::read_to_string(self.dir.path().join("out").join(name)).unwrap()
    }
}

#[test]
#[ignore = "boots a VM; see the top of the file"]
fn a_row_runs_in_a_fresh_vm() {
    let row = Row::sh(concat!(
        r#"printf %s "$IGNORARIUM_SEED" > /out/seed; cp /in/data.txt /out/copy; "#,
        "touch /in/x 2> /out/in_err; touch /usr/x 2> /out/usr_err; touch /x 2> /out/root_err; ",
        "date +%s > /out/time",
    ));
    let before = now_nanos();
    let o = row.run();
    let after = now_nanos();
    let st = &o.report.selftest;
    assert!(
        st.only_loopback && st.route_blocked && st.host_path_blocked,
        "{st:?}"
    );
    assert_eq!(o.report.end, Some(End::Code(0)));
    assert_eq!(o.output_hash, o.report.output_hash);
    assert_eq!(row.out("seed"), hex::encode(SEED));
    assert_eq!(row.out("copy"), "input bytes\n");
    for f in ["in_err", "usr_err", "root_err"] {
        assert!(!row.out(f).is_empty(), "{f} is empty, so the write worked");
    }
    // The guest clock is set to the host's when the job is sent, so both readings fall
    // inside the host's window around the row.
    let to_secs = |ns: u64| ns / 1_000_000_000;
    let date: u64 = row.out("time").trim().parse().unwrap();
    let (first, last) = (to_secs(before), to_secs(after));
    assert!(
        (first..=last).contains(&date),
        "guest date {date}, host {first}..{last}"
    );
    let clock = o.report.clock_unix_nanos;
    assert!(
        (before..=after).contains(&clock),
        "guest clock {clock}, host {before}..{after}"
    );
    assert!(
        o.timings.stop < row.vm.exit_timeout,
        "the guest did not power off: {:?}",
        o.timings
    );
}

#[test]
#[ignore = "boots a VM; see the top of the file"]
fn the_payload_view_is_the_documented_layout() {
    let row = Row::sh(concat!(
        "cd /; ls -A > /out/root; ls -A /dev > /out/dev; readlink /bin > /out/bin; ",
        "stat -c %a /tmp /dev/null > /out/modes; ls /proc/self/fd > /out/fds; ",
        // Mount point and the first per-mount option (ro or rw).
        "cut -d' ' -f5,6 /proc/self/mountinfo | sed 's/,.*//' | sort > /out/mounts; ",
        "head -c 17M /dev/zero > /out/big 2> /dev/null || { rm -f /out/big; exit 9; }",
    ));
    let o = row.run();
    assert_eq!(
        row.out("root"),
        "bin\ndev\netc\nin\nlib\nlib64\nout\nproc\ntmp\nusr\n"
    );
    assert_eq!(row.out("dev"), "full\nnull\nrandom\ntty\nurandom\nzero\n");
    assert_eq!(row.out("bin"), "usr/bin\n");
    assert_eq!(row.out("modes"), "1777\n666\n");
    // stdin, stdout, stderr and the listing's own directory: no descriptor of the agent.
    assert_eq!(row.out("fds"), "0\n1\n2\n3\n");
    assert_eq!(
        row.out("mounts"),
        "/ ro\n/dev rw\n/etc ro\n/in ro\n/out rw\n/proc rw\n/tmp rw\n/usr ro\n"
    );
    // A file larger than the 16 MiB output disk does not fit in /out.
    assert_eq!(o.report.end, Some(End::Code(9)));
}

#[test]
#[ignore = "boots a VM; see the top of the file"]
fn escaped_children_die_before_the_outputs_are_packed() {
    // The payload exits only once the child has left its session, so the group kill
    // cannot reach the child.
    let row = Row::sh(concat!(
        "setsid sh -c 'touch /out/escaped; sleep 1; touch /out/late' & ",
        "while [ ! -e /out/escaped ]; do sleep 0.05; done; exit 0",
    ));
    let o = row.run();
    assert_eq!(o.report.end, Some(End::Code(0)));
    assert_eq!(row.out("escaped"), "");
    assert!(!row.dir.path().join("out/late").exists());
    assert_eq!(o.output_hash, o.report.output_hash);
}

#[test]
#[ignore = "boots a VM; see the top of the file"]
fn exit_codes_and_timeouts_come_back() {
    let mut row = Row::sh("exit 7");
    // The default: no console at all.
    row.vm.serial_log = None;
    assert_eq!(row.run().report.end, Some(End::Code(7)));

    let mut row = Row::sh("sleep 60");
    row.job.wall_timeout = secs(2);
    let o = row.run();
    assert_eq!(o.report.end, Some(End::Timeout));
    assert!(o.timings.total < secs(20), "{:?}", o.timings);
}

#[test]
#[ignore = "boots a VM; see the top of the file"]
fn each_boot_draws_different_kernel_randomness() {
    // Both rows get the same seed. Mixing it in adds to the pool; it does not replace it.
    let draw = || {
        let row = Row::sh("head -c 16 /dev/urandom | od -An -tx1 > /out/r");
        row.run();
        row.out("r")
    };
    let (a, b) = (draw(), draw());
    assert_eq!(a.split_whitespace().count(), 16, "{a}");
    assert_ne!(a, b);
}

#[test]
#[ignore = "boots a VM; see the top of the file"]
fn python_from_the_environment_runs() {
    let row = Row::sh("python3 -c 'import numpy, sys; print(numpy.__version__)' > /out/v");
    let o = row.run();
    assert_eq!(o.report.end, Some(End::Code(0)));
    assert!(!row.out("v").trim().is_empty());
}

#[test]
#[ignore = "boots a VM; see the top of the file"]
fn errors_after_connect_come_back_as_failures() {
    let guest_failed = |row: &Row, want: &str| match row.try_run() {
        Err(VmError::Guest(m)) => assert!(m.contains(want), "{m}"),
        other => panic!("{other:?}"),
    };
    let mut row = Row::sh("");
    row.job.argv = vec!["/no/such/program".into()];
    guest_failed(&row, "No such file");
    // The file fills the 16 MiB output disk, so the pack, with its headers, does not fit.
    let row = Row::sh("head -c 16M /dev/zero > /out/big");
    guest_failed(&row, "No space left");
}
