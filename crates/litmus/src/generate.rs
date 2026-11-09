use sha2::{Digest, Sha256};
use std::fmt::Write as _;
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
    if !is_study_id(&shape.seed) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "seed must be lowercase letters and digits joined by single hyphens",
        ));
    }
    if [shape.pilot, shape.first, shape.reserve, shape.units].contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "pilot, first, reserve and units must each be at least 1",
        ));
    }
    let env = &shape.environment;
    if !env.is_empty()
        && !(env.len() == 64 && env.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "environment must be empty or 64 lowercase hex characters",
        ));
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
    let seed = &shape.seed;
    let (id, mode, label) = if confirmatory {
        (format!("litmus-{seed}"), "confirmatory", "confirmed")
    } else {
        (format!("litmus-pilot-{seed}"), "exploratory", "signal")
    };
    let mut s = String::new();
    let w = &mut s;
    let _ = writeln!(
        w,
        "[study]\nid = \"{id}\"\ntemplate = \"numeric-ablation\"\nmode = \"{mode}\"\nlabel = \"{label}\"\ntest_log = true\n"
    );
    let _ = writeln!(
        w,
        "[question]\npopulation = \"\"\nintervention = \"\"\ncomparator = \"\"\nnegative_control = \"\""
    );
    for (name, share) in CONDITIONS[1..].iter().zip(ALPHA_SHARES) {
        let _ = writeln!(
            w,
            "[[question.hypotheses]]\nid = \"{name}\"\nstatement = \"\"\nrival = \"\"\noutcome = \"y\"\ncontrast = {{ {name} = 1.0 }}\nstrata = []"
        );
        if confirmatory {
            let _ = writeln!(
                w,
                "claim = {{ below = 0.0 }}\nstatistic = {{ mean = [-1.0, 1.0] }}\npower_at = -0.6\nestimates = {{ from = \"litmus-pilot-{seed}\" }}\nalpha_share = {share}"
            );
        }
    }
    let _ = writeln!(
        w,
        "\n[[outcomes]]\nid = \"y\"\nunit = \"score\"\ndirection = \"lower\"\nmeasure = \"y\"\npaired = \"difference\"\nregistered = true\n"
    );
    let _ = writeln!(
        w,
        "[scope]\nmin_isolation = {}\nstop_rule = \"budget-only\"\n[scope.budget]\ncpu_hours = 1.0\nwall_hours = 1.0\nmoney_usd = 0.0\n[scope.end_of_run]\nroute = \"report\"\nfollow_on = []\n",
        shape.min_isolation
    );
    let _ = writeln!(
        w,
        "[design]\nunit = \"line\"\ncluster = \"file\"\nstratum = \"\"\nreference = \"ref\"\nrepeats = 1\nrow_seconds = 1.0\npower = 0.8\nunits_per_cluster = {}\npick = \"first\"",
        shape.units
    );
    let params: Vec<String> = [("work_ms", shape.work_ms), ("mem_mib", shape.mem_mib)]
        .iter()
        .filter(|(_, v)| *v != 0)
        .map(|(k, v)| format!("{k} = {v}"))
        .collect();
    for c in CONDITIONS {
        let _ = writeln!(w, "[[design.conditions]]\nid = \"{c}\"");
        if !params.is_empty() {
            let _ = writeln!(w, "params = {{ {} }}", params.join(", "));
        }
    }
    let _ = writeln!(
        w,
        "[design.confirmation]\nconditions = [\"good\", \"bad\", \"edge\"]"
    );
    if confirmatory {
        let _ = writeln!(w, "[design.resize]\nrule = \"blinded-once\"");
    }
    let _ = writeln!(w);
    for i in inputs {
        let _ = writeln!(
            w,
            "[[inputs]]\npath = \"{}\"\nsha256 = \"{}\"\nrole = \"{}\"\nunits = {}\n[inputs.licence]\nspdx = \"CC0-1.0\"\ntype = \"common\"\nsource = \"\"\ncreators = []\n",
            i.path, i.sha256, i.role, shape.units
        );
    }
    let _ = writeln!(
        w,
        "[pins]\nenvironment = \"{}\"\npayload = \"\"\nrule_base = \"\"\ndeterministic = true",
        shape.environment
    );
    if confirmatory && shape.environment.is_empty() {
        let _ = writeln!(
            w,
            "[[pins.deviations]]\nfrom = \"litmus-pilot-{seed}\"\nreason = \"the pilot ran on the host, which names no image\"\ntiming = \"before-collection\""
        );
    }
    let _ = writeln!(
        w,
        "\n[mutations]\ndepth_cap = 2\noperators = []\ntriggers = []\n[mutations.seeds]\nspawn = \"spawn/\"\nreport = \"report/\"\n\n[access]\nblind = {confirmatory}\n\n[report]\nlicence_code = \"Apache-2.0\"\nlicence_data = \"CC0-1.0\"\npublish = \"none\""
    );
    s
}
