mod flags;

use flags::Flags;
use litmus::{Shape, generate, payload, verify};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage:
  litmus gen --seed <s> --out <dir> [--pilot N] [--first N] [--reserve N] [--units N] [--work-ms N] [--mem-mib N] [--min-isolation N] [--environment <hash>]
  litmus verify --report <report.json> --expected <expected.json>
  litmus --condition <c> --unit <path>#<index> --repeat <n>
    (the payload: reads IGNORARIUM_IN, IGNORARIUM_OUT and IGNORARIUM_PARAMS)
  litmus --help | -h";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (name, result) = match args.split_first() {
        Some((cmd, _)) if cmd == "--help" || cmd == "-h" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some((cmd, rest)) if cmd == "gen" => ("litmus gen", gen_cmd(rest).map_err(|e| vec![e])),
        Some((cmd, rest)) if cmd == "verify" => ("litmus verify", verify_cmd(rest)),
        Some((cmd, _)) if cmd.starts_with("--") => {
            ("litmus", payload_cmd(&args).map_err(|e| vec![e]))
        }
        other => {
            if let Some((cmd, _)) = other {
                eprintln!("litmus: unknown command {cmd}");
            }
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(lines) => {
            lines.iter().for_each(|l| eprintln!("{name}: {l}"));
            ExitCode::FAILURE
        }
    }
}

fn gen_cmd(args: &[String]) -> Result<(), String> {
    let mut f = Flags::parse(args)?;
    let d = Shape::default();
    let shape = Shape {
        seed: f.take("--seed").unwrap_or_default(),
        pilot: f.number("--pilot", d.pilot)?,
        first: f.number("--first", d.first)?,
        reserve: f.number("--reserve", d.reserve)?,
        units: f.number("--units", d.units)?,
        work_ms: f.number("--work-ms", d.work_ms)?,
        mem_mib: f.number("--mem-mib", d.mem_mib)?,
        min_isolation: f.number("--min-isolation", d.min_isolation)?,
        environment: f.take("--environment").unwrap_or_default(),
    };
    let out = PathBuf::from(f.require("--out")?);
    f.done()?;
    let payload = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    generate(&shape, &payload, &out).map_err(|e| e.to_string())
}

fn verify_cmd(args: &[String]) -> Result<(), Vec<String>> {
    let mut f = Flags::parse(args).map_err(|e| vec![e])?;
    let mut read = |flag| -> Result<String, String> {
        let path = f.require(flag)?;
        std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))
    };
    let (report, expected) = (read("--report"), read("--expected"));
    f.done().map_err(|e| vec![e])?;
    match (report, expected) {
        (Ok(r), Ok(e)) => verify(&r, &e),
        (r, e) => Err([r, e].into_iter().filter_map(Result::err).collect()),
    }
}

fn payload_cmd(args: &[String]) -> Result<(), String> {
    let mut f = Flags::parse(args)?;
    let condition = f.require("--condition")?;
    let unit = f.require("--unit")?;
    // Every repeat of a row must give the same value, so the repeat index is checked and unused.
    f.number("--repeat", 0u32)?;
    f.done()?;
    let var = |name: &str| std::env::var_os(name).ok_or(format!("{name} is not set"));
    let params = match std::env::var("IGNORARIUM_PARAMS") {
        Ok(p) => p,
        Err(std::env::VarError::NotPresent) => "{}".into(),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err("IGNORARIUM_PARAMS is not UTF-8".into());
        }
    };
    payload(
        &condition,
        &unit,
        &PathBuf::from(var("IGNORARIUM_IN")?),
        &PathBuf::from(var("IGNORARIUM_OUT")?),
        &params,
    )
}
