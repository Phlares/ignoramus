//! An error from `accept` other than "no connection yet" ends the row at once.
//! It runs alone in its process: it lowers the file descriptor limit for all threads.
mod common;
use common::write_exec;
use ignorarium_guest::host::{HostJob, Vm, VmError, run_row};
use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
use std::fs::{self, File};
use std::os::unix::net::UnixStream;
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

#[test]
fn an_accept_error_surfaces_instead_of_waiting_for_the_boot_timeout() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    let (work, out, input) = (p.join("work"), p.join("out"), p.join("in"));
    for d in [&work, &out, &input] {
        fs::create_dir(d).unwrap();
    }
    let pidfile = p.join("vmm.pid");
    let vmm = p.join("fake-vmm");
    write_exec(
        &vmm,
        &format!(
            "#!/bin/sh\necho $$ > {}\nexec sleep 60\n",
            pidfile.display()
        ),
    );
    let vm = Vm {
        vmm,
        kernel: "kernel".into(),
        initramfs: "initramfs".into(),
        rootfs: "rootfs.img".into(),
        cpus: 2,
        mem_mib: 256,
        out_mib: 8,
        boot_timeout: Duration::from_secs(3),
        exit_timeout: Duration::from_millis(50),
        report_slack: Duration::from_secs(5),
        serial_log: None,
    };
    let job = HostJob {
        argv: vec!["/bin/prog".into()],
        env: vec![],
        input_dir: input,
        output_dir: out,
        seed: [0; 32],
        wall_timeout: Duration::from_secs(10),
    };
    let sock = work.join("vsock.sock_7201");
    let guest = thread::spawn(move || {
        let t = Instant::now();
        while fs::read_to_string(&pidfile)
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            assert!(
                t.elapsed() < Duration::from_secs(5),
                "the VMM never started"
            );
            sleep(Duration::from_millis(1));
        }
        // Leave exactly one free descriptor, spend it on the connect, and the host has none
        // for the connection it accepts.
        let saved = getrlimit(Resource::Nofile);
        let open = fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u64>().ok())
            .max()
            .unwrap();
        let lowered = Rlimit {
            current: Some(open + 2),
            maximum: saved.maximum,
        };
        setrlimit(Resource::Nofile, lowered).unwrap();
        let mut fill: Vec<File> = Vec::new();
        while let Ok(f) = File::open("/dev/null") {
            fill.push(f);
        }
        fill.pop();
        let conn = UnixStream::connect(&sock);
        sleep(Duration::from_millis(300));
        drop(fill);
        setrlimit(Resource::Nofile, saved).unwrap();
        conn.unwrap()
    });
    let t = Instant::now();
    let res = run_row(&vm, &job, &work);
    let _conn = guest.join().unwrap();
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    match res {
        Err(VmError::Io(e)) => assert_eq!(e.raw_os_error(), Some(24), "{e}"),
        other => panic!("{other:?}"),
    }
}
