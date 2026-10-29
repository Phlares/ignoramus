//! Boot time and per-row overhead of level 2.
//!
//! overhead [--runs N] [--cpus C] [--mem-mib M] [--env K=V]... [--baseline ARGV... --] -- PAYLOAD ARGV...
//!
//! Needs `/dev/kvm`, a static agent (`IGNORARIUM_VM_AGENT`), and `IGNORARIUM_VM_VMM`,
//! `IGNORARIUM_VM_KERNEL`, `IGNORARIUM_VM_ROOTFS` as in `tests/vm.rs`.
//!
//! Every sample is a fresh VM. The VM row and the baseline command alternate on the same
//! machine: the VM goes first on even runs, the baseline first on odd runs. Run 0 is printed
//! and dropped from the summary. Method: Georges, Buytaert, Eeckhout 2007
//! (https://dri.es/files/oopsla07-georges.pdf); Laaber, Scheuner, Leitner 2019
//! (https://research.chalmers.se/publication/511491/file/511491_Fulltext.pdf); Kalibera, Jones 2013
//! (https://kar.kent.ac.uk/33611/45/p63-kaliber.pdf).
//! `ci95` is the half-width of the 95% Student t interval of the mean, so the mean is `mean ± ci95`.
use ignorarium_guest::host::{HostJob, REPORT_SLACK, Timings, Vm, run_row};
use ignorarium_guest::image::initramfs;
use ignorarium_guest::proto::End;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Student t quantiles at 0.975 for 1 to 30 degrees of freedom.
const T975: [f64; 30] = [
    12.706, 4.303, 3.182, 2.776, 2.571, 2.447, 2.365, 2.306, 2.262, 2.228, 2.201, 2.179, 2.160,
    2.145, 2.131, 2.120, 2.110, 2.101, 2.093, 2.086, 2.080, 2.074, 2.069, 2.064, 2.060, 2.056,
    2.052, 2.048, 2.045, 2.042,
];

#[derive(Debug, Serialize)]
struct Summary {
    n: usize,
    mean: f64,
    ci95: f64,
    median: f64,
    min: f64,
    max: f64,
}

/// Mean with a 95% Student t half-width (1.96 above 30 degrees of freedom). One sample has no
/// interval, so `ci95` is 0 there. `xs` must not be empty.
fn summary(xs: &[f64]) -> Summary {
    let n = xs.len();
    let mean = xs.iter().sum::<f64>() / n as f64;
    let ci95 = if n < 2 {
        0.0
    } else {
        let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
        let t = T975.get(n - 2).copied().unwrap_or(1.96);
        t * (var / n as f64).sqrt()
    };
    let mut s = xs.to_vec();
    s.sort_by(f64::total_cmp);
    let median = (s[(n - 1) / 2] + s[n / 2]) / 2.0;
    Summary {
        n,
        mean,
        ci95,
        median,
        min: s[0],
        max: s[n - 1],
    }
}

struct Args {
    runs: usize,
    cpus: u32,
    mem_mib: u32,
    env: Vec<(String, String)>,
    baseline: Option<Vec<String>>,
    payload: Vec<String>,
}

const USAGE: &str = "usage: overhead [--runs N] [--cpus C] [--mem-mib M] [--env K=V]... \
    [--baseline ARGV... --] -- PAYLOAD ARGV...";

fn parse(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut a = Args {
        runs: 31,
        cpus: 1,
        mem_mib: 1024,
        env: vec![],
        baseline: None,
        payload: vec![],
    };
    let mut it = args;
    while let Some(flag) = it.next() {
        let value =
            |it: &mut dyn Iterator<Item = String>| it.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--runs" => {
                a.runs = value(&mut it)?
                    .parse()
                    .map_err(|e| format!("--runs: {e}"))?
            }
            "--cpus" => {
                a.cpus = value(&mut it)?
                    .parse()
                    .map_err(|e| format!("--cpus: {e}"))?
            }
            "--mem-mib" => {
                a.mem_mib = value(&mut it)?
                    .parse()
                    .map_err(|e| format!("--mem-mib: {e}"))?
            }
            "--env" => {
                let kv = value(&mut it)?;
                let (k, v) = kv
                    .split_once('=')
                    .ok_or(format!("--env wants K=V, got {kv}"))?;
                a.env.push((k.into(), v.into()));
            }
            "--baseline" => {
                let argv: Vec<String> = it.by_ref().take_while(|s| s != "--").collect();
                if it.next().as_deref() != Some("--") {
                    return Err("--baseline ARGV... -- must be followed by -- PAYLOAD".into());
                }
                a.baseline = Some(argv);
                a.payload = it.by_ref().collect();
                break;
            }
            "--" => {
                a.payload = it.by_ref().collect();
                break;
            }
            _ => return Err(format!("unknown flag {flag}")),
        }
    }
    if a.payload.is_empty() || a.baseline.as_ref().is_some_and(|b| b.is_empty()) || a.runs < 2 {
        return Err(
            "need a payload, a non-empty baseline if given, and --runs of at least 2".into(),
        );
    }
    Ok(a)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn var(name: &str) -> Result<PathBuf, String> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .ok_or(format!("set {name}"))
}

fn baseline_ms(argv: &[String]) -> Result<f64, String> {
    let t = Instant::now();
    let st = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("baseline: {e}"))?;
    let took = ms(t.elapsed());
    if st.success() {
        Ok(took)
    } else {
        Err(format!("baseline exited with {st}"))
    }
}

/// The value of the first line that starts with `key` (any line if `key` is empty).
fn first_line(path: &str, key: &str) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .find(|l| l.starts_with(key))
        .map(|l| l.split_once(':').map_or(l, |(_, v)| v).trim().to_string())
        .unwrap_or_default()
}

fn fields(t: &Timings) -> [(&'static str, f64); 7] {
    [
        ("prepare", ms(t.prepare)),
        ("boot", ms(t.boot)),
        ("guest", ms(t.guest)),
        ("payload", ms(t.payload)),
        ("stop", ms(t.stop)),
        ("extract", ms(t.extract)),
        ("total", ms(t.total)),
    ]
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a = parse(std::env::args().skip(1)).map_err(|e| format!("{e}\n{USAGE}"))?;
    let tmp = tempfile::tempdir()?;
    let agent = std::fs::read(var("IGNORARIUM_VM_AGENT")?)?;
    std::fs::write(tmp.path().join("initramfs.cpio"), initramfs(&agent))?;
    let serial = tmp.path().join("serial.log");
    let vm = Vm {
        vmm: var("IGNORARIUM_VM_VMM")?,
        kernel: var("IGNORARIUM_VM_KERNEL")?,
        initramfs: tmp.path().join("initramfs.cpio"),
        rootfs: var("IGNORARIUM_VM_ROOTFS")?,
        cpus: a.cpus,
        mem_mib: a.mem_mib,
        out_mib: 64,
        boot_timeout: Duration::from_secs(30),
        exit_timeout: Duration::from_secs(10),
        report_slack: REPORT_SLACK,
        serial_log: Some(serial.clone()),
    };
    let mut env = vec![(
        "PATH".to_string(),
        "/usr/local/bin:/usr/bin:/bin".to_string(),
    )];
    env.extend(a.env.iter().cloned());

    let vm_once = |run: usize| -> Result<Timings, String> {
        let dir = tempfile::tempdir_in(tmp.path()).map_err(|e| e.to_string())?;
        for d in ["in", "out", "work"] {
            std::fs::create_dir(dir.path().join(d)).map_err(|e| e.to_string())?;
        }
        let job = HostJob {
            argv: a.payload.clone(),
            env: env.clone(),
            input_dir: dir.path().join("in"),
            output_dir: dir.path().join("out"),
            seed: [7; 32],
            wall_timeout: Duration::from_secs(120),
        };
        let o = run_row(&vm, &job, &dir.path().join("work")).map_err(|e| {
            let console = std::fs::read_to_string(&serial).unwrap_or_default();
            format!("run {run}: {e}\nguest console:\n{console}")
        })?;
        match o.report.end {
            Some(End::Code(0)) => Ok(o.timings),
            end => Err(format!("run {run}: payload ended with {end:?}")),
        }
    };

    let mut kept: Vec<(Timings, Option<f64>)> = vec![];
    for run in 0..a.runs {
        let vm_first = run % 2 == 0;
        let (mut t, mut b) = (None, None);
        for step in 0..2 {
            if (step == 0) == vm_first {
                t = Some(vm_once(run)?);
            } else if let Some(argv) = &a.baseline {
                b = Some(baseline_ms(argv)?);
            }
        }
        let t = t.expect("every run boots one VM");
        let order = if vm_first {
            "vm_first"
        } else {
            "baseline_first"
        };
        let mut line = json!({ "run": run, "order": order });
        for (k, v) in fields(&t) {
            line[k] = json!(v);
        }
        line["baseline_ms"] = json!(b);
        println!("{line}");
        if run > 0 {
            kept.push((t, b));
        }
    }

    let mut series: Vec<(String, Vec<f64>)> = fields(&Timings::default())
        .iter()
        .enumerate()
        .map(|(i, (k, _))| {
            (
                k.to_string(),
                kept.iter().map(|(t, _)| fields(t)[i].1).collect(),
            )
        })
        .collect();
    if a.baseline.is_some() {
        let base: Vec<f64> = kept.iter().map(|(_, b)| b.unwrap()).collect();
        let diff = |name: &str| -> Vec<f64> {
            let i = series.iter().position(|(k, _)| k == name).unwrap();
            series[i].1.iter().zip(&base).map(|(x, b)| x - b).collect()
        };
        let (dt, dp) = (diff("total"), diff("payload"));
        series.extend([
            ("baseline_ms".into(), base),
            ("overhead_total_minus_baseline".into(), dt),
            ("overhead_payload_minus_baseline".into(), dp),
        ]);
    }
    let stats: serde_json::Map<String, Value> = series
        .iter()
        .map(|(k, xs)| (k.clone(), serde_json::to_value(summary(xs)).unwrap()))
        .collect();
    println!(
        "{}",
        json!({
            "summary": stats,
            "unit": "ms",
            "runs_kept": kept.len(),
            "cpus": a.cpus,
            "mem_mib": a.mem_mib,
            "payload": a.payload,
            "baseline": a.baseline,
            "cpu_model": first_line("/proc/cpuinfo", "model name"),
            "kernel": first_line("/proc/sys/kernel/osrelease", ""),
        })
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_samples_use_the_t_quantile_for_four_degrees() {
        let s = summary(&[1., 2., 3., 4., 5.]);
        assert_eq!(
            (s.n, s.mean, s.median, s.min, s.max),
            (5, 3.0, 3.0, 1.0, 5.0)
        );
        assert!(
            (s.ci95 - 2.776 * 1.5811 / 5f64.sqrt()).abs() < 1e-3,
            "{}",
            s.ci95
        );
    }

    #[test]
    fn even_length_median_is_the_middle_mean() {
        assert_eq!(summary(&[4., 1., 3., 2.]).median, 2.5);
    }

    #[test]
    fn forty_samples_use_1_96() {
        let xs: Vec<f64> = (0..40).map(f64::from).collect();
        let sd = (40.0f64 * 41.0 / 12.0).sqrt();
        assert!((summary(&xs).ci95 - 1.96 * sd / 40f64.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn thirty_one_samples_use_the_last_table_entry() {
        let xs: Vec<f64> = (0..31).map(f64::from).collect();
        let sd = (31.0f64 * 32.0 / 12.0).sqrt();
        assert!((summary(&xs).ci95 - 2.042 * sd / 31f64.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn one_sample_has_no_interval() {
        let s = summary(&[7.0]);
        assert_eq!((s.n, s.mean, s.ci95, s.median), (1, 7.0, 0.0, 7.0));
    }

    #[test]
    fn arguments_split_baseline_and_payload() {
        let a = parse(
            [
                "--runs",
                "5",
                "--env",
                "A=b=c",
                "--baseline",
                "x",
                "-y",
                "--",
                "--",
                "p",
                "--q",
            ]
            .map(String::from)
            .into_iter(),
        )
        .unwrap();
        assert_eq!(a.runs, 5);
        assert_eq!(a.env, [("A".to_string(), "b=c".to_string())]);
        assert_eq!(a.baseline.unwrap(), ["x", "-y"]);
        assert_eq!(a.payload, ["p", "--q"]);
        assert!(parse(["--runs", "5"].map(String::from).into_iter()).is_err());
    }
}
