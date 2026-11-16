//! The file a stamp covers: the box's lines for one study, built from its verified log.
use crate::canon::sha256_hex;
use crate::derived::Register;
use crate::files::{self, WriteOnceError, write_once};
use crate::home::Home;
use crate::records::{self, VerifyError};
use ed25519_dalek::VerifyingKey;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum StampError {
    Io(io::Error),
    /// The log does not verify.
    Log(VerifyError),
    /// The study has no register record.
    NotRegistered,
    /// The stamped file could not be written.
    Write(WriteOnceError),
}

impl std::fmt::Display for StampError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StampError::Io(e) => write!(f, "I/O failed: {e}"),
            StampError::Log(e) => write!(f, "{e}"),
            StampError::NotRegistered => write!(f, "the study has no register record"),
            StampError::Write(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for StampError {}

impl From<io::Error> for StampError {
    fn from(e: io::Error) -> Self {
        StampError::Io(e)
    }
}

impl From<VerifyError> for StampError {
    fn from(e: VerifyError) -> Self {
        StampError::Log(e)
    }
}

/// The lines the box writes. Each starts with its prefix; the value follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Register,
    RegisterSignature,
    Key,
    Head,
    HeadSignature,
}

impl Line {
    pub fn prefix(self, id: &str) -> String {
        match self {
            Line::Register => format!("register record hash of {id}: "),
            Line::RegisterSignature => format!("register record signature of {id}: "),
            Line::Key => "box public key (ed25519): ".into(),
            Line::Head => format!("record head of {id}: "),
            Line::HeadSignature => format!("record head signature of {id}: "),
        }
    }
}

/// The box's lines for study `id`, LF line ends. The plan log head line is left out when
/// `heads.jsonl` is missing or empty.
pub fn lines(home: &Home, id: &str, key: &VerifyingKey) -> Result<String, StampError> {
    let path = home.study_dir(id).join("records.jsonl");
    let seals = records::seals(&path, key)?;
    let register = records::first(&path, key)?
        .as_ref()
        .and_then(Register::of)
        .ok_or(StampError::NotRegistered)?;
    let (first, head) = match (seals.first(), seals.last()) {
        (Some(f), Some(h)) => (f, h),
        _ => return Err(StampError::NotRegistered),
    };
    let mut text = String::new();
    let mut push = |line: Line, value: &str| {
        text.push_str(&line.prefix(id));
        text.push_str(value);
        text.push('\n');
    };
    push(Line::Register, &first.hash);
    push(Line::RegisterSignature, &first.sig);
    push(Line::Key, &hex::encode(key.to_bytes()));
    push(Line::Head, &format!("seq {}, hash {}", head.seq, head.hash));
    push(Line::HeadSignature, &head.sig);
    let heads = home.root.join(&register.log).join("heads.jsonl");
    if let Some(body) = files::optional(fs::read_to_string(heads))?
        && let Some(last) = body.lines().rfind(|l| !l.is_empty())
    {
        text.push_str(&format!("plan log head ({}): {last}\n", register.log));
    }
    Ok(text)
}

/// Write the file to stamp, `lines` then the bytes of `with`, to `<study>/stamps/<sha256>.txt`.
/// The same log gives the same path.
pub fn request(
    home: &Home,
    id: &str,
    key: &VerifyingKey,
    with: Option<&Path>,
) -> Result<PathBuf, StampError> {
    let mut bytes = lines(home, id, key)?.into_bytes();
    if let Some(extra) = with {
        bytes.extend(fs::read(extra)?);
    }
    let dir = stamps_dir(&home.study_dir(id))?;
    let file = dir.join(format!("{}.txt", sha256_hex(&bytes)));
    write_once(&file, &bytes, 0o644).map_err(StampError::Write)?;
    Ok(file)
}

/// `<study>/stamps`, made private when absent. A link or a file there is refused.
fn stamps_dir(study: &Path) -> Result<PathBuf, StampError> {
    let dir = study.join("stamps");
    let mut b = fs::DirBuilder::new();
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    match b.create(&dir) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e.into()),
        _ => {}
    }
    if !fs::symlink_metadata(&dir)?.is_dir() {
        return Err(io::Error::other(format!("{} is not a folder", dir.display())).into());
    }
    Ok(dir)
}
