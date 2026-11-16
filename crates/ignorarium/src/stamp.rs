//! The file a stamp covers, the box's lines for one study from its verified log, and the
//! record of an outside time stamp of that file.
use crate::canon::{canonical_of, sha256_hex};
use crate::derived::{By, Register, STAMP, STAMP_CHECKED, Stamp, StampLines};
use crate::files::{WriteOnceError, write_once};
use crate::home::Home;
use crate::planlog::store::{LogError, PlanLog};
use crate::records::{self, Record, RecordDraft, RecordError, Seal, VerifyError};
use crate::tsp;
use ed25519_dalek::{SigningKey, VerifyingKey};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum StampError {
    Io(io::Error),
    /// The log does not verify.
    Log(VerifyError),
    /// The plan log the study registered in does not verify.
    PlanLog {
        log: String,
        cause: LogError,
    },
    /// The study has no register record.
    NotRegistered,
    /// The stamped file could not be written.
    Write(WriteOnceError),
    /// The record could not be appended.
    Append(RecordError),
    /// A reply or file the rule `code` refuses; `found` says what was found.
    Refused {
        code: &'static str,
        found: String,
    },
}

impl std::fmt::Display for StampError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StampError::Io(e) => write!(f, "I/O failed: {e}"),
            StampError::Log(e) => write!(f, "{e}"),
            StampError::PlanLog { log, cause } => write!(f, "plan log {log}: {cause}"),
            StampError::NotRegistered => write!(f, "the study has no register record"),
            StampError::Write(e) => write!(f, "{e}"),
            StampError::Append(e) => write!(f, "{e}"),
            StampError::Refused { code, found } => write!(f, "{code}: {found}"),
        }
    }
}

impl From<RecordError> for StampError {
    fn from(e: RecordError) -> Self {
        StampError::Append(e)
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

fn refused(code: &'static str, found: String) -> StampError {
    StampError::Refused { code, found }
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

/// The box's lines for study `id`, LF line ends. The plan log head is the last head of the
/// verified plan log, as canonical JSON; its line is left out when the log has no head.
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
    // `PlanLog::open` makes a missing folder and its lock file; a request must make neither.
    let folder = home.root.join(&register.log);
    let plan = if folder.is_dir() {
        (PlanLog::open(&folder).and_then(|l| l.verified_head(key))).map_err(|cause| {
            StampError::PlanLog {
                log: register.log.clone(),
                cause,
            }
        })?
    } else {
        None
    };
    if let Some(head) = plan {
        let json = String::from_utf8(canonical_of(&head)?).map_err(io::Error::other)?;
        text.push_str(&format!("plan log head ({}): {json}\n", register.log));
    }
    Ok(text)
}

/// Which of the box's lines `file` holds, checked against the log's `seals` and the box key.
/// A line with a box line's prefix and another value is refused, so `true` rests only on a
/// value the box computed. The head line is checked against the record it names, which may
/// be older than the log's last.
pub fn found(
    file: &[u8],
    id: &str,
    seals: &[Seal],
    key_hex: &str,
) -> Result<StampLines, StampError> {
    let first = seals.first().ok_or(StampError::NotRegistered)?;
    let shown = |line: Line, value: &[u8]| {
        format!(
            "{:?}",
            format!("{}{}", line.prefix(id), String::from_utf8_lossy(value))
        )
    };
    // The one value the file gives `line`; two different values are refused.
    let value = |line: Line| -> Result<Option<&[u8]>, StampError> {
        let prefix = line.prefix(id);
        let mut values = file
            .split(|&b| b == b'\n')
            .filter_map(|l| l.strip_prefix(prefix.as_bytes()));
        let one = values.next();
        match (one, values.find(|v| Some(*v) != one)) {
            (Some(a), Some(b)) => Err(refused(
                "E-STP-003",
                format!("two lines {} and {}", shown(line, a), shown(line, b)),
            )),
            _ => Ok(one),
        }
    };
    let holds = |line: Line, want: &str| -> Result<bool, StampError> {
        match value(line)? {
            None => Ok(false),
            Some(v) if v == want.as_bytes() => Ok(true),
            Some(v) => Err(refused(
                "E-STP-003",
                format!(
                    "the line {} is not the box's line {}",
                    shown(line, v),
                    shown(line, want.as_bytes())
                ),
            )),
        }
    };
    if !holds(Line::Register, &first.hash)? {
        return Err(refused(
            "E-STP-003",
            format!("no line {}", shown(Line::Register, first.hash.as_bytes())),
        ));
    }
    let head = match value(Line::Head)? {
        None => None,
        Some(v) => Some(head_seal(v, seals).ok_or_else(|| {
            refused(
                "E-STP-004",
                format!(
                    "the line {} names no record of the log",
                    shown(Line::Head, v)
                ),
            )
        })?),
    };
    let head_signature = match head {
        Some(seal) => holds(Line::HeadSignature, &seal.sig)?,
        None => match value(Line::HeadSignature)? {
            None => false,
            Some(v) => {
                return Err(refused(
                    "E-STP-003",
                    format!(
                        "the line {} with no head line",
                        shown(Line::HeadSignature, v)
                    ),
                ));
            }
        },
    };
    Ok(StampLines {
        register: true,
        register_signature: holds(Line::RegisterSignature, &first.sig)?,
        key: holds(Line::Key, key_hex)?,
        head: head.map(|s| By {
            seq: s.seq,
            hash: s.hash.clone(),
        }),
        head_signature,
    })
}

/// The seal a head value `seq <n>, hash <hex>` names, when the box would write that value.
fn head_seal<'a>(value: &[u8], seals: &'a [Seal]) -> Option<&'a Seal> {
    let value = std::str::from_utf8(value).ok()?;
    let seq: usize = value
        .strip_prefix("seq ")?
        .split_once(',')?
        .0
        .parse()
        .ok()?;
    let seal = seals.get(seq)?;
    (format!("seq {}, hash {}", seal.seq, seal.hash) == value).then_some(seal)
}

/// What `record` did.
#[derive(Debug)]
pub enum Stamped {
    New(Record),
    /// A record of this reply is in the log already.
    Already,
}

/// Record the reply at `reply` as a `stamp` record of study `id`. The stamped file is `file`,
/// else `stamps/<digest>.txt`. Every check comes before the first write; the copies in
/// `stamps/` are write-once, and the record is appended unless one holds this reply.
pub fn record(
    home: &Home,
    id: &str,
    key: &SigningKey,
    reply: &Path,
    file: Option<&Path>,
) -> Result<Stamped, StampError> {
    let der = fs::read(reply)?;
    let parsed = tsp::parse(&der).map_err(|e| refused("E-STP-001", e.to_string()))?;
    if !parsed.granted {
        return Err(refused("E-STP-001", "the reply is not granted".into()));
    }
    if parsed.digest_alg != tsp::SHA256 {
        return Err(refused(
            "E-STP-001",
            format!("digest algorithm {}", parsed.digest_alg),
        ));
    }
    if parsed.digest.len() != 32 {
        return Err(refused(
            "E-STP-001",
            format!("digest length {} bytes, not 32", parsed.digest.len()),
        ));
    }
    let digest = hex::encode(&parsed.digest);
    let study = home.study_dir(id);
    let path = file.map_or_else(
        || study.join("stamps").join(format!("{digest}.txt")),
        Path::to_path_buf,
    );
    let bytes = match fs::read(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(refused("E-STP-002", format!("no file {}", path.display())));
        }
        r => r?,
    };
    let file_sha256 = sha256_hex(&bytes);
    if file_sha256 != digest {
        return Err(refused(
            "E-STP-002",
            format!(
                "{} has SHA-256 {file_sha256}, the reply {digest}",
                path.display()
            ),
        ));
    }
    let log = study.join("records.jsonl");
    let vk = key.verifying_key();
    records::first(&log, &vk)?
        .as_ref()
        .and_then(Register::of)
        .ok_or(StampError::NotRegistered)?;
    let seals = records::seals(&log, &vk)?;
    let lines = found(&bytes, id, &seals, &hex::encode(vk.to_bytes()))?;
    let reply_sha256 = sha256_hex(&der);
    let dir = stamps_dir(&study)?;
    write_once(&dir.join(format!("{file_sha256}.txt")), &bytes, 0o644)
        .map_err(StampError::Write)?;
    write_once(&dir.join(format!("{reply_sha256}.tsr")), &der, 0o644).map_err(StampError::Write)?;
    let body = Stamp {
        file_sha256,
        reply_sha256,
        authority: parsed.authority,
        stamped_at: parsed.stamped_at,
        serial: parsed.serial,
        policy: parsed.policy,
        lines,
        checked: STAMP_CHECKED.into(),
    };
    let draft = RecordDraft::new(
        id,
        STAMP,
        serde_json::to_value(&body).map_err(io::Error::other)?,
    );
    let mut new = records::append_with(&log, key, |log| {
        let held =
            (log.records.iter().filter_map(Stamp::of)).any(|s| s.reply_sha256 == body.reply_sha256);
        Ok::<_, StampError>(if held { Vec::new() } else { vec![draft] })
    })?;
    Ok(new.pop().map_or(Stamped::Already, Stamped::New))
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
