use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::Path;

/// The size and settings of a generated study.
pub struct Shape {
    pub seed: String,
    pub pilot: u32,
    pub first: u32,
    pub reserve: u32,
    pub units: u32,
    pub work_ms: u32,
    pub mem_mib: u32,
    pub min_isolation: u8,
    /// The environment pin of both specs. The box refuses an empty pin at isolation 2.
    pub environment: String,
}

impl Default for Shape {
    fn default() -> Self {
        Shape {
            seed: String::new(),
            pilot: 8,
            first: 12,
            reserve: 60,
            units: 2,
            work_ms: 0,
            mem_mib: 0,
            min_isolation: 0,
            environment: String::new(),
        }
    }
}

/// The conditions of the study. The first is the reference.
const CONDITIONS: [&str; 4] = ["ref", "good", "bad", "edge"];
const ALPHA_SHARES: [&str; 3] = ["0.34", "0.33", "0.33"];

/// A pinned data file: its path, SHA-256 and the role the spec gives it.
struct Input {
    path: String,
    sha256: String,
    role: &'static str,
}

/// Writes `count` files named `<batch>-<nnnn>.txt`. Each holds one line per unit.
/// The seed and batch name are in every line, so no two studies or batches share a file.
fn batch(
    shape: &Shape,
    out: &Path,
    name: &str,
    count: u32,
    role: &'static str,
) -> io::Result<Vec<Input>> {
    (1..=count)
        .map(|n| {
            let text: String = (0..shape.units)
                .map(|u| format!("{} {name} {n:04} {u}\n", shape.seed))
                .collect();
            let path = format!("data/{name}-{n:04}.txt");
            write(out, &path, text.as_bytes())?;
            let sha256 = hex::encode(Sha256::digest(text.as_bytes()));
            Ok(Input { path, sha256, role })
        })
        .collect()
}

fn write(out: &Path, path: &str, bytes: &[u8]) -> io::Result<()> {
    let file = out.join(path);
    fs::create_dir_all(file.parent().expect("every path has a folder"))?;
    fs::write(file, bytes)
}

/// Writes a complete study under `out`: data, payload, two specs and the expected verdicts.
pub fn generate(shape: &Shape, out: &Path) -> io::Result<()> {
    if let Some(why) = refusal(shape, out) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, why));
    }
    let pilot = batch(shape, out, "pilot", shape.pilot, "confirmation")?;
    let mut litmus = batch(shape, out, "first", shape.first, "confirmation")?;
    litmus.extend(batch(shape, out, "reserve", shape.reserve, "reserve")?);

    write(out, "ignorarium.toml", b"")?;
    write(
        out,
        "spec/pilot.toml",
        spec(shape, false, &pilot).as_bytes(),
    )?;
    write(
        out,
        "spec/litmus.toml",
        spec(shape, true, &litmus).as_bytes(),
    )?;
    write(
        out,
        "expected.json",
        br#"{"good":"pass","bad":"fail","edge":"fail"}"#,
    )?;
    copy_payload(&out.join("payload/run"))
}

/// Why `generate` refuses `shape` or `out`, before it writes anything. A study in an occupied
/// folder could sit beside stale files from an earlier, larger study.
fn refusal(shape: &Shape, out: &Path) -> Option<&'static str> {
    let env = &shape.environment;
    let hex = env.len() == 64 && env.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !is_study_id(&shape.seed) {
        Some("seed must be lowercase letters and digits joined by single hyphens")
    } else if [shape.pilot, shape.first, shape.reserve, shape.units].contains(&0) {
        Some("pilot, first, reserve and units must each be at least 1")
    } else if !env.is_empty() && !hex {
        Some("environment must be empty or 64 lowercase hex characters")
    } else if fs::read_dir(out).is_ok_and(|mut d| d.next().is_some()) {
        Some("out must be absent or an empty folder")
    } else {
        None
    }
}

/// The box's id pattern `^[a-z0-9]+(-[a-z0-9]+)*$`. The seed is part of both study ids.
fn is_study_id(s: &str) -> bool {
    s.split('-').all(|p| {
        !p.is_empty()
            && p.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

fn copy_payload(to: &Path) -> io::Result<()> {
    fs::create_dir_all(to.parent().expect("every path has a folder"))?;
    fs::copy(std::env::current_exe()?, to)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(to, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// One spec. The pilot is exploratory and makes no claim. The litmus study is confirmatory.
fn spec(shape: &Shape, confirmatory: bool, inputs: &[Input]) -> String {
    let Shape {
        seed,
        units,
        min_isolation,
        environment,
        ..
    } = shape;
    let (id, mode, label) = if confirmatory {
        (format!("litmus-{seed}"), "confirmatory", "confirmed")
    } else {
        (format!("litmus-pilot-{seed}"), "exploratory", "signal")
    };
    let hypotheses: String = (CONDITIONS[1..].iter().zip(ALPHA_SHARES))
        .map(|(name, share)| {
            let claim = if confirmatory {
                format!(
                    r#"claim = {{ below = 0.0 }}
statistic = {{ mean = [-1.0, 1.0] }}
power_at = -0.6
estimates = {{ from = "litmus-pilot-{seed}" }}
alpha_share = {share}
"#
                )
            } else {
                String::new()
            };
            format!(
                r#"[[question.hypotheses]]
id = "{name}"
statement = ""
rival = ""
outcome = "y"
contrast = {{ {name} = 1.0 }}
strata = []
{claim}"#
            )
        })
        .collect();
    let params: Vec<String> = [("work_ms", shape.work_ms), ("mem_mib", shape.mem_mib)]
        .iter()
        .filter(|(_, v)| *v != 0)
        .map(|(k, v)| format!("{k} = {v}"))
        .collect();
    let params = if params.is_empty() {
        String::new()
    } else {
        format!("params = {{ {} }}\n", params.join(", "))
    };
    let conditions: String = (CONDITIONS.iter())
        .map(|c| format!("[[design.conditions]]\nid = \"{c}\"\n{params}"))
        .collect();
    let resize = if confirmatory {
        "[design.resize]\nrule = \"blinded-once\"\n"
    } else {
        ""
    };
    let inputs: String = (inputs.iter())
        .map(|Input { path, sha256, role }| {
            format!(
                r#"[[inputs]]
path = "{path}"
sha256 = "{sha256}"
role = "{role}"
units = {units}
[inputs.licence]
spdx = "CC0-1.0"
type = "common"
source = ""
creators = []

"#
            )
        })
        .collect();
    let deviation = if confirmatory && environment.is_empty() {
        format!(
            r#"[[pins.deviations]]
from = "litmus-pilot-{seed}"
reason = "the pilot ran on the host, which names no image"
timing = "before-collection"
"#
        )
    } else {
        String::new()
    };
    format!(
        r#"[study]
id = "{id}"
template = "numeric-ablation"
mode = "{mode}"
label = "{label}"
test_log = true

[question]
population = ""
intervention = ""
comparator = ""
negative_control = ""
{hypotheses}
[[outcomes]]
id = "y"
unit = "score"
direction = "lower"
measure = "y"
paired = "difference"
registered = true

[scope]
min_isolation = {min_isolation}
stop_rule = "budget-only"
[scope.budget]
cpu_hours = 1.0
wall_hours = 1.0
money_usd = 0.0
[scope.end_of_run]
route = "report"
follow_on = []

[design]
unit = "line"
cluster = "file"
stratum = ""
reference = "ref"
repeats = 1
row_seconds = 1.0
power = 0.8
units_per_cluster = {units}
pick = "first"
{conditions}[design.confirmation]
conditions = ["good", "bad", "edge"]
{resize}
{inputs}[pins]
environment = "{environment}"
payload = ""
rule_base = ""
deterministic = true
{deviation}
[mutations]
depth_cap = 2
operators = []
triggers = []
[mutations.seeds]
spawn = "spawn/"
report = "report/"

[access]
blind = {confirmatory}

[report]
licence_code = "Apache-2.0"
licence_data = "CC0-1.0"
publish = "none"
"#
    )
}
