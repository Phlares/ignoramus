use ignorarium::backend::{Backend, Exit, Probe, RowJob, RowOutcome, Watch, level0::Level0};
use ignorarium::files::replace_atomic;
use ignorarium::machine::{self, Machine};

struct Fake(u8, bool);
impl Backend for Fake {
    fn level(&self) -> u8 {
        self.0
    }
    fn probe(&self) -> Probe {
        if self.1 {
            Probe::Available
        } else {
            Probe::Unavailable("no".into())
        }
    }
    fn run_row(&self, _: &RowJob, _: &dyn Watch) -> std::io::Result<RowOutcome> {
        Ok(RowOutcome {
            exit: Exit::Ok,
            cpu_seconds: 0.0,
            wall_seconds: 0.0,
            output_hash: String::new(),
            selftest_ok: true,
        })
    }
}

#[test]
fn probe_reports_the_highest_available_level() {
    let d = tempfile::tempdir().unwrap();
    let down: Vec<Box<dyn Backend>> = vec![Box::new(Level0), Box::new(Fake(1, false))];
    assert_eq!(machine::probe(d.path(), &down).unwrap().max_level, 0);
    let up: Vec<Box<dyn Backend>> = vec![Box::new(Level0), Box::new(Fake(1, true))];
    assert_eq!(machine::probe(d.path(), &up).unwrap().max_level, 1);
    let skip: Vec<Box<dyn Backend>> = vec![Box::new(Fake(2, false)), Box::new(Fake(1, true))];
    assert_eq!(machine::probe(d.path(), &skip).unwrap().max_level, 1);
}

#[test]
fn probe_fills_host_facts() {
    let d = tempfile::tempdir().unwrap();
    let b: Vec<Box<dyn Backend>> = vec![Box::new(Level0)];
    let m = machine::probe(d.path(), &b).unwrap();
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap();
    assert_eq!(m.name, host.trim());
    assert_eq!(
        m.cpus as usize,
        std::thread::available_parallelism().unwrap().get()
    );
    assert!(m.mem_gib >= 1);
    let def = Machine::default();
    assert_eq!(m.rate_usd_per_hour, def.rate_usd_per_hour);
    assert_eq!(m.long_job_minutes, def.long_job_minutes);
    assert_eq!(m.experiment_mode, def.experiment_mode);
}

#[test]
fn probe_keeps_a_configured_rate() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("machine.toml"),
        "rate_usd_per_hour = 0.5\nexperiment_mode = true\nlong_job_minutes = 7\nmax_level = 0\n",
    )
    .unwrap();
    let b: Vec<Box<dyn Backend>> = vec![Box::new(Level0), Box::new(Fake(1, true))];
    let m = machine::probe(d.path(), &b).unwrap();
    assert_eq!(m.max_level, 1);
    assert_eq!(m.rate_usd_per_hour, 0.5);
    assert!(m.experiment_mode);
    assert_eq!(m.long_job_minutes, 7);
}

#[test]
fn meminfo_parses_whole_gib_with_a_floor_of_one() {
    assert_eq!(
        machine::mem_gib_of("MemTotal:  16777216 kB\nMemFree: 1 kB\n"),
        Some(16)
    );
    assert_eq!(machine::mem_gib_of("MemTotal:  2097151 kB\n"), Some(1));
    assert_eq!(machine::mem_gib_of("MemTotal:  100 kB\n"), Some(1));
    assert_eq!(machine::mem_gib_of("MemFree: 1 kB\n"), None);
    assert_eq!(machine::mem_gib_of("MemTotal: lots kB\n"), None);
}

#[test]
fn write_round_trips_through_load() {
    let d = tempfile::tempdir().unwrap();
    let m = Machine {
        name: "box".into(),
        cpus: 4,
        mem_gib: 8,
        max_level: 1,
        rate_usd_per_hour: 0.25,
        experiment_mode: true,
        long_job_minutes: 12,
    };
    machine::write(d.path(), &m).unwrap();
    assert_eq!(machine::load(d.path()).unwrap(), m);
    machine::write(d.path(), &Machine::default()).unwrap();
    assert_eq!(machine::load(d.path()).unwrap(), Machine::default());
}

#[test]
fn replace_atomic_overwrites_and_refuses_a_symlink() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("f");
    replace_atomic(&p, b"one", 0o600).unwrap();
    replace_atomic(&p, b"two", 0o600).unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"two");
    assert_eq!(
        std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let target = d.path().join("t");
    std::fs::write(&target, b"keep").unwrap();
    let ln = d.path().join("ln");
    std::os::unix::fs::symlink(&target, &ln).unwrap();
    assert!(replace_atomic(&ln, b"bad", 0o600).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    assert!(
        std::fs::symlink_metadata(&ln)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn replace_atomic_leaves_no_temp_on_failure() {
    let d = tempfile::tempdir().unwrap();
    let dir_target = d.path().join("sub");
    std::fs::create_dir(&dir_target).unwrap();
    std::fs::write(dir_target.join("x"), b"x").unwrap();
    assert!(replace_atomic(&dir_target, b"bad", 0o600).is_err());
    let names: Vec<_> = std::fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["sub".to_string()]);
}

#[test]
fn probe_with_no_backends_gives_level_zero() {
    let d = tempfile::tempdir().unwrap();
    let none: Vec<Box<dyn Backend>> = vec![];
    assert_eq!(machine::probe(d.path(), &none).unwrap().max_level, 0);
}

#[test]
fn probe_refuses_a_bad_machine_file_and_leaves_it_alone() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("machine.toml");
    let bad = "rate_usd_per_hour = -1.0
";
    std::fs::write(&f, bad).unwrap();
    let b: Vec<Box<dyn Backend>> = vec![Box::new(Level0)];
    let e = machine::probe(d.path(), &b).unwrap_err();
    assert!(e.to_string().contains("rate_usd_per_hour"), "{e}");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), bad);
    std::fs::write(
        &f,
        "no_such_field = 1
",
    )
    .unwrap();
    assert!(machine::probe(d.path(), &b).is_err());
}
