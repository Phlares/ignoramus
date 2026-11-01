//! Rows in fresh Cloud Hypervisor VMs. They are ignored by default: they need `/dev/kvm`, an
//! agent built static (see `STATIC_BUILD`) and three files named by environment variables:
//! `IGNORARIUM_VM_VMM` (the VMM binary), `IGNORARIUM_VM_KERNEL` (its guest kernel) and
//! `IGNORARIUM_VM_ROOTFS` (an ext4 image of the environment: a merged-usr root with `sh`,
//! coreutils, `setsid` and Python with numpy).
mod common;
use common::write_exec;
use ignorarium_guest::agent::steps::now_nanos;
use ignorarium_guest::host::{
    HostJob, REPORT_SLACK, Vm, VmError, VmOutcome, needed_mem_mib, run_row,
};
use ignorarium_guest::image::{has_interp, initramfs, newc_entry};
use ignorarium_guest::proto::End;
use std::fs;
use std::os::unix::fs::PermissionsExt;
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

struct Row {
    dir: tempfile::TempDir,
    vm: Vm,
    job: HostJob,
}

impl Row {
    /// A row that runs `script` with `/bin/sh -c`. The input dir holds `data.txt`; the
    /// payload dir starts empty.
    fn sh(script: &str) -> Row {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        for d in ["in", "out", "work", "payload"] {
            fs::create_dir(p.join(d)).unwrap();
        }
        // The payload runs as another user, so both trees must be open to others.
        for d in ["in", "payload"] {
            fs::set_permissions(p.join(d), fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(p.join("in/data.txt"), "input bytes\n").unwrap();
        let agent = Path::new(env!("CARGO_BIN_EXE_ignorarium-guest"));
        let bytes = fs::read(agent).unwrap();
        assert!(
            !has_interp(&bytes).unwrap(),
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
            payload_dir: p.join("payload"),
            output_dir: p.join("out"),
            seed: SEED,
            wall_timeout: secs(60),
            data_limit: None,
        };
        Row { dir, vm, job }
    }

    fn try_run(&self) -> Result<VmOutcome, VmError> {
        let res = run_row(&self.vm, &self.job, &self.dir.path().join("work"), &|| true);
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

/// One more newc cpio archive, holding `pad` as a file of `bytes` zero bytes. The kernel
/// unpacks every archive in the initramfs, so the file takes guest memory that the host's
/// size check does not count.
fn padding(bytes: usize) -> Vec<u8> {
    let mut out = Vec::new();
    newc_entry(&mut out, 9, "pad", 0o100644, (0, 0), &vec![0; bytes]);
    newc_entry(&mut out, 0, "TRAILER!!!", 0, (0, 0), &[]);
    out
}

#[test]
#[ignore = "boots a VM; see the top of the file"]
fn a_guest_without_room_for_the_row_refuses_it_before_the_payload() {
    let mut row = Row::sh("touch /out/ran");
    row.job.data_limit = Some(640 << 20);
    // The least machine the host accepts for this row, with 256 MiB more held by the guest.
    let need = needed_mem_mib(row.job.data_limit, row.vm.out_mib);
    row.vm.mem_mib = u32::try_from(need).unwrap();
    let mut cpio = fs::read(&row.vm.initramfs).unwrap();
    cpio.extend(padding(256 << 20));
    fs::write(&row.vm.initramfs, cpio).unwrap();
    match row.try_run() {
        Err(VmError::Guest(m)) => {
            let want = "MiB available at boot, under the 792 MiB the row needs: a data limit \
                        of 640 MiB, 24 MiB for /out and /tmp, and a 128 MiB floor";
            assert!(m.contains(want), "{m}");
        }
        other => panic!("{other:?}"),
    }
    assert!(!row.dir.path().join("out/ran").exists());
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
    // The agent switches io_uring off, so the guest kernel refuses it to every process.
    assert_eq!(st.io_uring_errno, Some(libc::EPERM), "{st:?}");
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
        // Mount point and per-mount options.
        "cut -d' ' -f5,6 /proc/self/mountinfo | sort > /out/mounts; ",
        "if head -c 17M /dev/zero > /tmp/big 2> /dev/null; then echo fits; else echo capped; fi ",
        "> /out/tmp_cap; rm -f /tmp/big; ",
        "head -c 17M /dev/zero > /out/big 2> /dev/null || { rm -f /out/big; exit 9; }",
    ));
    let o = row.run();
    assert_eq!(
        row.out("root"),
        "bin\ndev\netc\nin\nlib\nlib64\nout\npayload\nproc\ntmp\nusr\n"
    );
    assert_eq!(row.out("dev"), "full\nnull\nrandom\ntty\nurandom\nzero\n");
    assert_eq!(row.out("bin"), "usr/bin\n");
    assert_eq!(row.out("modes"), "1777\n666\n");
    // stdin, stdout, stderr and the listing's own directory: no descriptor of the agent.
    assert_eq!(row.out("fds"), "0\n1\n2\n3\n");
    let mounts = [
        "/ ro,relatime",
        "/dev rw,nosuid,noexec,relatime",
        "/etc ro,nosuid,nodev,relatime",
        "/in ro,nosuid,nodev,relatime",
        "/out rw,nosuid,nodev,relatime",
        "/payload ro,nosuid,nodev,relatime",
        "/proc rw,nosuid,nodev,noexec,relatime",
        "/tmp rw,nosuid,nodev,relatime",
        "/usr ro,nosuid,nodev,relatime",
    ];
    assert_eq!(
        row.out("mounts"),
        mounts.map(|m| m.to_owned() + "\n").concat()
    );
    // A file larger than the 16 MiB output disk fits neither in /tmp nor in /out.
    assert_eq!(row.out("tmp_cap"), "capped\n");
    assert_eq!(o.report.end, Some(End::Code(9)));
}

#[test]
#[ignore = "boots a VM; see the top of the file"]
fn the_payload_has_no_privileges() {
    let row = Row::sh(concat!(
        "id -u > /out/uid; id -g > /out/gid; id -G > /out/groups; ",
        "grep NoNewPrivs /proc/self/status > /out/nnp; ",
        "if [ -e /proc/1 ]; then echo seen; else echo hidden; fi > /out/pid1; ",
        "ls /proc/1/fd > /dev/null 2>&1; echo $? > /out/fd1; ",
        "mount -t tmpfs none /tmp 2> /dev/null; echo $? > /out/mount; ",
        "touch /out/x",
    ));
    let o = row.run();
    assert_eq!(o.report.end, Some(End::Code(0)));
    for (f, want) in [
        ("uid", "65534\n"),
        ("gid", "65534\n"),
        ("groups", "65534\n"),
    ] {
        assert_eq!(row.out(f), want, "{f}");
    }
    assert_eq!(row.out("nnp"), "NoNewPrivs:\t1\n");
    assert_eq!(row.out("pid1"), "hidden\n");
    assert_ne!(row.out("fd1"), "0\n", "PID 1's descriptors are listable");
    // 127 would mean no `mount` program, so the check would prove nothing.
    let mount: i32 = row.out("mount").trim().parse().unwrap();
    assert!(![0, 127].contains(&mount), "mount exited {mount}");
    assert_eq!(row.out("x"), "");
}

#[test]
#[ignore = "boots a VM; see the top of the file"]
fn the_outputs_are_on_the_disk_when_the_report_arrives() {
    // The host kills the VMM the moment the report arrives, so the outputs are there only
    // if the agent synced them before it reported.
    let mut row = Row::sh("head -c 4M /dev/urandom > /out/r; echo done > /out/small");
    row.vm.exit_timeout = Duration::ZERO;
    let o = row.run();
    assert_eq!(o.report.end, Some(End::Code(0)));
    assert_eq!(o.output_hash, o.report.output_hash);
    let r = fs::metadata(row.dir.path().join("out/r")).unwrap();
    assert_eq!(r.len(), 4 << 20);
    assert_eq!(row.out("small"), "done\n");
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
}

#[test]
#[ignore = "boots a VM; see the top of the file"]
fn the_payload_folder_is_read_only_at_slash_payload() {
    let mut row = Row::sh("");
    let script = concat!(
        "#!/bin/sh\n",
        "printf %s \"$0\" > /out/argv0\n",
        "if touch /payload/x 2> /dev/null; then echo wrote; else echo refused; fi > /out/touch\n",
    );
    write_exec(&row.job.payload_dir.join("run"), script);
    row.job.argv = vec!["/payload/run".into()];
    let o = row.run();
    assert_eq!(o.report.end, Some(End::Code(0)));
    assert_eq!(row.out("argv0"), "/payload/run");
    assert_eq!(row.out("touch"), "refused\n");
}

#[test]
#[ignore = "boots a VM; see the top of the file"]
fn the_payload_starts_in_out() {
    // As at level 1, so a payload that writes a relative path works at both levels.
    let row = Row::sh("pwd > /out/pwd; echo rel > relative");
    let o = row.run();
    assert_eq!(o.report.end, Some(End::Code(0)));
    assert_eq!(
        row.out("pwd"),
        "/out
"
    );
    assert_eq!(
        row.out("relative"),
        "rel
"
    );
}

#[test]
#[ignore = "boots a VM; see the top of the file"]
fn an_output_tree_past_path_max_is_the_payloads_result() {
    // 21 folders of 200 bytes, made with relative calls: the walk cannot name the deepest.
    // A shell's `cd` goes by the whole path, so Python makes them.
    let o = Row::sh(
        "python3 -c 'import os\nos.chdir(\"/out\")\nfor _ in range(21):\n    \
         os.mkdir(\"0\" * 200)\n    os.chdir(\"0\" * 200)\nopen(\"f\", \"w\").write(\"x\")'",
    )
    .run();
    assert_eq!(o.report.end, Some(End::Code(0)));
    let why = o.report.pack_refused.unwrap_or_default();
    assert!(why.starts_with("\"an output path is too long: "), "{why}");
    assert_eq!(o.output_hash, "");
}

#[test]
#[ignore = "boots a VM; see the top of the file"]
fn a_sparse_output_is_the_payloads_result() {
    // A sparse file and hard links take less of /out than they take of the pack: the pack
    // refuses them, and the payload's own run stands.
    for script in [
        "truncate -s 1G /out/sparse",
        "head -c 6M /dev/zero > /out/a; ln /out/a /out/b; ln /out/a /out/c",
    ] {
        let o = Row::sh(script).run();
        assert_eq!(o.report.end, Some(End::Code(0)), "{script}");
        assert_eq!(
            o.report.pack_refused.as_deref(),
            Some("\"outputs exceed the output limit of 16777216 bytes\""),
            "{script}"
        );
        assert_eq!(o.output_hash, "", "{script}");
    }
}
