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
    /// tightened to 0700).
    pub fn open_at(root: &Path) -> Result<Home, HomeError> {
        if running_under_wsl() && is_windows_mount(root) {
            return Err(HomeError::WindowsMount(root.to_path_buf()));
        }
        for name in SUBDIRS {
            let dir = root.join(name);
            fs::create_dir_all(&dir)?;
            #[cfg(unix)]
            fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
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

fn running_under_wsl() -> bool {
    fs::read_to_string("/proc/sys/kernel/osrelease")
        .is_ok_and(|s| s.to_ascii_lowercase().contains("microsoft"))
}
