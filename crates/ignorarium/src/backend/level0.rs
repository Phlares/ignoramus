//! Level 0: a bare process on the host. No isolation.
use super::{Backend, Exit, Probe, RowJob, RowOutcome, Watch, outcome, supervise};
use std::io;
use std::process::{Command, Stdio};
use std::time::Instant;

pub struct Level0;

impl Backend for Level0 {
    fn level(&self) -> u8 {
        0
    }

    fn probe(&self) -> Probe {
        Probe::Available
    }

    fn launcher(&self) -> String {
        "process".into()
    }

    fn run_row(&self, job: &RowJob, watch: &dyn Watch) -> io::Result<RowOutcome> {
        let start = Instant::now();
        let (exit, cpu_seconds) = run_child(job, start, watch);
        Ok(outcome(job, start, exit, cpu_seconds))
    }
}

fn run_child(job: &RowJob, start: Instant, watch: &dyn Watch) -> (Exit, f64) {
    let Some((prog, args)) = job.argv.split_first() else {
        return (Exit::Infra("empty argv".into()), 0.0);
    };
    let mut cmd = Command::new(prog);
    cmd.args(args)
        .env_clear()
        .envs(job.env.iter().map(|(k, v)| (k, v)))
        .env("IGNORARIUM_IN", &job.input_dir)
        .env("IGNORARIUM_OUT", &job.output_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    supervise(cmd, job, start, watch)
}
