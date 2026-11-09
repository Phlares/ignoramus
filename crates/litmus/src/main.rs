use litmus::{Shape, generate};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: litmus gen --seed <s> --out <dir> [--pilot N] [--first N] [--reserve N] [--units N] [--work-ms N] [--mem-mib N] [--min-isolation N] [--environment <hash>]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((cmd, rest)) if cmd == "gen" => match gen_cmd(rest) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("litmus gen: {e}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn gen_cmd(args: &[String]) -> Result<(), String> {
    let mut shape = Shape::default();
    let mut out = None;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let val = it.next().ok_or(format!("{flag} needs a value"))?;
        let num = || val.parse::<u32>().map_err(|e| format!("{flag}: {e}"));
        match flag.as_str() {
            "--seed" => shape.seed = val.clone(),
            "--out" => out = Some(PathBuf::from(val)),
            "--environment" => shape.environment = val.clone(),
            "--pilot" => shape.pilot = num()?,
            "--first" => shape.first = num()?,
            "--reserve" => shape.reserve = num()?,
            "--units" => shape.units = num()?,
            "--work-ms" => shape.work_ms = num()?,
            "--mem-mib" => shape.mem_mib = num()?,
            "--min-isolation" => {
                shape.min_isolation = val.parse().map_err(|e| format!("{flag}: {e}"))?
            }
            _ => return Err(format!("unknown flag {flag}")),
        }
    }
    let out = out.ok_or("--out is required")?;
    generate(&shape, &out).map_err(|e| e.to_string())
}
