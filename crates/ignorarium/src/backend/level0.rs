//! Level 0: a bare process on the host. No isolation.
use super::cgroup::RowCgroup;
use super::{
    Backend, Exit, Probe, RowJob, RowOutcome, Supervised, Watch, host_cpus, outcome, supervise,
};
use crate::sys;
use crate::telemetry::Reading;
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

    fn cpus(&self) -> u32 {
        host_cpus()
    }

    fn run_row(&self, job: &RowJob, watch: &dyn Watch) -> io::Result<RowOutcome> {
        Ok(self.run_in(job, watch, RowCgroup::make))
    }
}

impl Level0 {
    /// Run `job` in the cgroup `leaf` makes, or in the box's own when it gives a reason.
    pub fn run_in(
        &self,
        job: &RowJob,
        watch: &dyn Watch,
        leaf: impl FnOnce() -> Result<RowCgroup, String>,
    ) -> RowOutcome {
        let leaf = leaf();
        let start = Instant::now();
        let s = run_child(job, start, watch, leaf.as_ref().ok());
        let o = outcome(job, start, s.exit, s.cpu_seconds, s.ended);
        RowOutcome {
            cgroup: leaf.map_or_else(Reading::Unavailable, RowCgroup::finish),
            ..o
        }
    }
}

fn run_child(
    job: &RowJob,
    start: Instant,
    watch: &dyn Watch,
    leaf: Option<&RowCgroup>,
) -> Supervised {
    let Some((prog, args)) = job.argv.split_first() else {
        return Supervised {
            exit: Exit::Infra("empty argv".into()),
            cpu_seconds: 0.0,
            ended: false,
        };
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
    sys::limit_data(&mut cmd, job.data_limit());
    if let Some(leaf) = leaf {
        leaf.enter(&mut cmd);
    }
    supervise(cmd, job, start, watch)
}
