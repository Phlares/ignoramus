//! Safe folder walk. Links and special files are refused, never followed.
use crate::Problem;
use std::fs::{self, File};
use std::io;
use std::path::Path;

const INSTRUCTION: &str = "Replace links and special files with regular files, and rename files to UTF-8 names without control characters or backslashes.";

fn refuse(path: String, message: &str) -> Problem {
    Problem::new("E-PUB-001", path, message, INSTRUCTION)
}

/// Controls, C1 controls and bidi or format marks: a terminal or reader may act on them.
fn is_hidden(c: char) -> bool {
    matches!(c,
        '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{61c}' | '\u{200e}' | '\u{200f}'
        | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

pub(crate) fn name_is_safe(name: &str) -> bool {
    !name.chars().any(|c| is_hidden(c) || c == '\\')
}

/// Text for display: hidden chars as `\u{..}` escapes and a backslash as `\\`.
pub fn shown(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\\' => "\\\\".to_string(),
            c if is_hidden(c) => c.escape_unicode().to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// Device and inode of a file. Empty where the platform gives none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Id {
    #[cfg(unix)]
    dev_ino: (u64, u64),
}

impl Id {
    fn of(m: &fs::Metadata) -> Id {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Id {
                dev_ino: (m.dev(), m.ino()),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = m;
            Id {}
        }
    }
}

/// A regular file the walk saw: its path, length and identity at that moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: String,
    pub len: u64,
    id: Id,
}

/// Opens a found file once and checks the handle is the file the walk saw.
/// A swap for a link, another file or a resize gives `InvalidData`.
pub fn open(dir: &Path, f: &Found) -> io::Result<File> {
    let file = File::open(dir.join(&f.path))?;
    let m = file.metadata()?;
    if m.is_file() && Id::of(&m) == f.id && m.len() == f.len {
        Ok(file)
    } else {
        Err(changed(&f.path))
    }
}

pub(crate) fn changed(path: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{} changed while the bundle was read", shown(path)),
    )
}

/// Lists regular files as `/`-joined relative paths, sorted by bytes.
/// Every refused entry is one E-PUB-001 problem.
pub fn walk(dir: &Path) -> Result<(Vec<Found>, Vec<Problem>), io::Error> {
    let mut files = Vec::new();
    let mut problems = Vec::new();
    visit(dir, "", &mut files, &mut problems)?;
    files.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    problems.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((files, problems))
}

fn visit(
    dir: &Path,
    prefix: &str,
    files: &mut Vec<Found>,
    problems: &mut Vec<Problem>,
) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let raw = entry.file_name();
        let (name, valid) = match raw.to_str() {
            Some(s) => (s.to_string(), name_is_safe(s)),
            None => (raw.to_string_lossy().into_owned(), false),
        };
        let rel = format!("{prefix}{name}");
        if !valid {
            problems.push(refuse(shown(&rel), "name cannot be held in a bundle"));
            continue;
        }
        let meta = fs::symlink_metadata(entry.path())?;
        let kind = meta.file_type();
        if kind.is_symlink() {
            problems.push(refuse(rel, "is a symbolic link"));
        } else if kind.is_dir() {
            visit(&entry.path(), &format!("{rel}/"), files, problems)?;
        } else if kind.is_file() {
            files.push(Found {
                path: rel,
                len: meta.len(),
                id: Id::of(&meta),
            });
        } else {
            problems.push(refuse(rel, "is not a regular file or folder"));
        }
    }
    Ok(())
}
