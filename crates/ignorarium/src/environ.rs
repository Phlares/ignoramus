//! Run-environment identity: a recipe hash, a container build whose saved
//! image goes into the content store, and a measured rebuild rate.
use crate::canon::{sha256_hex, to_canonical};
use crate::store::{Store, StoreError, is_digest};
use serde_json::json;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const DOCKERFILE: &str = "Dockerfile";
const LOCKFILE: &str = "uv.lock";
/// Longest build-error tail kept in `EnvError::Build`.
const TAIL: usize = 2000;

#[derive(Debug)]
pub enum EnvError {
    NoBuilder,
    Build(String),
    Io(io::Error),
}

impl std::fmt::Display for EnvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnvError::NoBuilder => write!(f, "neither docker nor podman is on PATH"),
            EnvError::Build(m) => write!(f, "image build failed: {m}"),
            EnvError::Io(e) => write!(f, "I/O failed: {e}"),
        }
    }
}

impl std::error::Error for EnvError {}

impl From<io::Error> for EnvError {
    fn from(e: io::Error) -> Self {
        EnvError::Io(e)
    }
}

impl From<StoreError> for EnvError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Io(e) => EnvError::Io(e),
            other => EnvError::Io(io::Error::other(other.to_string())),
        }
    }
}

/// The two files that define an environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recipe {
    pub dockerfile: String,
    pub lockfile: String,
}

impl Recipe {
    /// Reads `Dockerfile` and `uv.lock` from `dir`.
    pub fn load(dir: &Path) -> io::Result<Recipe> {
        Ok(Recipe {
            dockerfile: std::fs::read_to_string(dir.join(DOCKERFILE))?,
            lockfile: std::fs::read_to_string(dir.join(LOCKFILE))?,
        })
    }
}

/// Hash of the recipe bytes. It names the inputs, not the built image.
pub fn recipe_hash(r: &Recipe) -> String {
    let v = json!({
        "dockerfile": sha256_hex(r.dockerfile.as_bytes()),
        "lockfile": sha256_hex(r.lockfile.as_bytes()),
    });
    // A `Value` built from strings always serializes.
    sha256_hex(&to_canonical(&v).unwrap_or_default())
}

/// True when the Dockerfile has a `FROM` and every `FROM` names an earlier
/// stage, `scratch` (the empty image), or an image pinned as `@sha256:<64 lowercase hex>`.
pub fn check_pinned(dockerfile: &str) -> bool {
    let joined = dockerfile.replace("\\\r\n", " ").replace("\\\n", " ");
    let mut stages: Vec<String> = Vec::new();
    let mut seen = false;
    for line in joined.lines() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some(w) if w.eq_ignore_ascii_case("from") => {}
            _ => continue,
        }
        let mut rest = words.skip_while(|w| w.starts_with("--"));
        let Some(image) = rest.next() else {
            return false;
        };
        seen = true;
        let ok = image.eq_ignore_ascii_case("scratch")
            || stages.contains(&image.to_ascii_lowercase())
            || (!image.contains('$')
                && image
                    .split_once("@sha256:")
                    .is_some_and(|(_, d)| is_digest(d)));
        if !ok {
            return false;
        }
        if let (Some(a), Some(name)) = (rest.next(), rest.next())
            && a.eq_ignore_ascii_case("as")
        {
            stages.push(name.to_ascii_lowercase());
        }
    }
    seen
}

/// A container engine: the program to run.
#[derive(Clone, Debug)]
pub struct Builder {
    program: PathBuf,
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

impl Builder {
    pub fn docker() -> Builder {
        Builder::with_program("docker")
    }

    pub fn podman() -> Builder {
        Builder::with_program("podman")
    }

    pub fn with_program(p: impl Into<PathBuf>) -> Builder {
        Builder { program: p.into() }
    }

    /// Docker first, then Podman, by `PATH`; `None` when neither is present.
    pub fn detect() -> Option<Builder> {
        if on_path("docker") {
            Some(Builder::docker())
        } else if on_path("podman") {
            Some(Builder::podman())
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    pub recipe_hash: String,
    /// Image id from `--iidfile`, 64 hex digits.
    pub image_digest: String,
    /// Hash of the saved image tar in the store.
    pub store_digest: String,
    /// The Dockerfile pins every base by digest. The caller warns when false.
    pub pinned: bool,
}

/// Reads a small text file and refuses a symbolic link at `p`.
fn read_nofollow(p: &Path) -> io::Result<String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let mut text = String::new();
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(p)?
        .take(4096)
        .read_to_string(&mut text)?;
    Ok(text)
}

fn tail(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let start = s.char_indices().rev().nth(TAIL - 1).map_or(0, |(i, _)| i);
    s[start..].trim().to_string()
}

/// Builds the image. Returns the id as the engine prints it and as 64 hex digits.
fn build_image(
    b: &Builder,
    dir: &Path,
    store: &Store,
    no_cache: bool,
) -> Result<(String, String), EnvError> {
    let mut rnd = [0u8; 8];
    getrandom::fill(&mut rnd).map_err(|e| io::Error::other(e.to_string()))?;
    // The store root is private (0700); the random name is unguessable.
    let iid = store
        .root
        .join(format!(".{}.{}.iid", std::process::id(), hex::encode(rnd)));
    let mut cmd = Command::new(&b.program);
    cmd.arg("build").arg("--iidfile").arg(&iid);
    if no_cache {
        cmd.arg("--no-cache");
    }
    cmd.arg("-f").arg(dir.join(DOCKERFILE)).arg(dir);
    let out = cmd.stdin(Stdio::null()).output().map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            EnvError::NoBuilder
        } else {
            EnvError::Io(e)
        }
    })?;
    let text = read_nofollow(&iid);
    // Scratch file this call created; a missing one is fine.
    let _ = std::fs::remove_file(&iid);
    if !out.status.success() {
        return Err(EnvError::Build(tail(&out.stderr)));
    }
    let raw = text?;
    let id = raw.trim();
    let hex = id.strip_prefix("sha256:").unwrap_or(id);
    if !is_digest(hex) {
        return Err(EnvError::Build(format!("unexpected image id: {id:?}")));
    }
    Ok((id.to_string(), hex.to_string()))
}

pub fn build(b: &Builder, recipe_dir: &Path, store: &Store) -> Result<Built, EnvError> {
    let recipe = Recipe::load(recipe_dir)?;
    let (id, image_digest) = build_image(b, recipe_dir, store, false)?;
    let mut save = Command::new(&b.program)
        .arg("save")
        .arg(&id)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let Some(mut stdout) = save.stdout.take() else {
        return Err(EnvError::Build("no stdout from image save".into()));
    };
    let put = store.put_reader(&mut stdout);
    drop(stdout);
    let status = save.wait()?;
    let store_digest = put?;
    if !status.success() {
        return Err(EnvError::Build(format!("image save exited with {status}")));
    }
    Ok(Built {
        recipe_hash: recipe_hash(&recipe),
        image_digest,
        store_digest,
        pinned: check_pinned(&recipe.dockerfile),
    })
}

/// Builds `n` times without cache. `store` only holds the scratch id files. Returns the fraction of builds whose
/// image id equals the first build's. `n == 0` is an error.
pub fn rebuild_rate(
    b: &Builder,
    recipe_dir: &Path,
    store: &Store,
    n: u32,
) -> Result<f64, EnvError> {
    if n == 0 {
        return Err(EnvError::Build("rebuild count must be at least 1".into()));
    }
    let first = build_image(b, recipe_dir, store, true)?.1;
    let mut same = 1u32;
    for _ in 1..n {
        if build_image(b, recipe_dir, store, true)?.1 == first {
            same += 1;
        }
    }
    Ok(f64::from(same) / f64::from(n))
}
