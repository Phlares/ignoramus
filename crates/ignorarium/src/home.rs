//! The box home: state folders under `IGNORARIUM_HOME` or `~/.ignorarium`.
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

const SUBDIRS: [&str; 4] = ["keys", "planlog", "testlog", "studies"];

#[derive(Debug)]
pub enum HomeError {
    /// No `IGNORARIUM_HOME` and no home directory.
    NoHome,
    /// Running under WSL with the home on a Windows drive.
    WindowsMount(PathBuf),
    /// The root or a state folder is a symbolic link.
    Link(PathBuf),
    Io(io::Error),
}

impl std::fmt::Display for HomeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HomeError::NoHome => write!(f, "set IGNORARIUM_HOME: no home directory found"),
            HomeError::WindowsMount(p) => write!(
                f,
                "{} is on a Windows drive; set IGNORARIUM_HOME to a Linux path",
                p.display()
            ),
            HomeError::Link(p) => write!(f, "{} is a symbolic link", p.display()),
            HomeError::Io(e) => write!(f, "I/O failed: {e}"),
        }
    }
}

impl std::error::Error for HomeError {}

impl From<io::Error> for HomeError {
    fn from(e: io::Error) -> Self {
        HomeError::Io(e)
    }
}

#[derive(Debug, Clone)]
pub struct Home {
    pub root: PathBuf,
}

impl Home {
    pub fn open() -> Result<Home, HomeError> {
        let root = match std::env::var_os("IGNORARIUM_HOME") {
            Some(p) if !p.is_empty() => PathBuf::from(p),
            _ => std::env::home_dir()
                .ok_or(HomeError::NoHome)?
                .join(".ignorarium"),
        };
        Home::open_at(&root)
    }

    /// Create the state folders (mode 0700 on unix; existing ones are
    /// tightened to 0700). A symbolic link as root or subfolder is an error.
    pub fn open_at(root: &Path) -> Result<Home, HomeError> {
        if let Ok(release) = fs::read_to_string("/proc/sys/kernel/osrelease")
            && refuse_mount(&release, root)
        {
            return Err(HomeError::WindowsMount(root.to_path_buf()));
        }
        make_private_dir(root, true)?;
        for name in SUBDIRS {
            make_private_dir(&root.join(name), false)?;
        }
        Ok(Home {
            root: root.to_path_buf(),
        })
    }

    pub fn study_dir(&self, id: &str) -> PathBuf {
        self.root.join("studies").join(id)
    }
}

/// True for `/mnt/<one letter>` and anything below it.
pub fn is_windows_mount(path: &Path) -> bool {
    let mut parts = path.components();
    if parts.next() != Some(Component::RootDir) {
        return false;
    }
    if parts.next() != Some(Component::Normal("mnt".as_ref())) {
        return false;
    }
    match parts.next() {
        Some(Component::Normal(d)) => {
            let b = d.as_encoded_bytes();
            b.len() == 1 && b[0].is_ascii_alphabetic()
        }
        _ => false,
    }
}

/// Create `dir` (and parents when `parents`) with mode 0700; tighten an
/// existing one. Refuses a symbolic link.
pub(crate) fn make_private_dir(dir: &Path, parents: bool) -> Result<(), HomeError> {
    match fs::symlink_metadata(dir) {
        Ok(m) if m.file_type().is_symlink() => return Err(HomeError::Link(dir.to_path_buf())),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut b = fs::DirBuilder::new();
            b.recursive(parents);
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
            b.create(dir)?;
        }
        Err(e) => return Err(e.into()),
    }
    #[cfg(unix)]
    fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    Ok(())
}

/// True when `osrelease` names WSL and `root` lands on a Windows drive
/// (`/mnt/<one letter>`). `root` is made absolute, its nearest existing
/// ancestor is resolved through symlinks, and `..` is folded away.
pub fn refuse_mount(osrelease: &str, root: &Path) -> bool {
    osrelease.to_ascii_lowercase().contains("microsoft") && is_windows_mount(&resolve(root))
}

fn resolve(path: &Path) -> PathBuf {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut rest = Vec::new();
    let mut base = abs.as_path();
    let mut resolved = loop {
        if let Ok(c) = base.canonicalize() {
            break c;
        }
        match (base.parent(), base.file_name()) {
            (Some(p), Some(n)) => {
                rest.push(n.to_owned());
                base = p;
            }
            (Some(p), None) => {
                rest.push("..".into());
                base = p;
            }
            _ => break base.to_path_buf(),
        }
    };
    for part in rest.into_iter().rev() {
        resolved.push(part);
    }
    let mut out = PathBuf::new();
    for c in resolved.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}
