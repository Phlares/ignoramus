//! The payload's view: the tree the agent enters with `chroot` before it starts the payload
//! as `PAYLOAD_UID`:`PAYLOAD_GID`.
//!
//! - `usr bin lib lib64 etc` come from the environment: a directory is bound read-only,
//!   nosuid, nodev; a symbolic link (as in merged-usr images) is copied as the same link; a
//!   missing one is left out;
//! - `/in` is the input disk: read-only, nosuid, nodev;
//! - `/out` is a tmpfs (nosuid, nodev) owned by the payload's user, mode 0755, capped at the
//!   output disk's size; the agent packs it onto that disk;
//! - `/proc` with `hidepid=2`, so the payload sees only its own user's processes (nosuid,
//!   nodev, noexec);
//! - `/dev` is a tmpfs (nosuid, noexec) with `null zero full random urandom tty`, mode 0666;
//! - `/tmp` is a tmpfs (nosuid, nodev), mode 1777.
//!
//! The root is a tmpfs, remounted read-only once it holds all of the above.
use super::{PAYLOAD_GID, PAYLOAD_UID};
use rustix::fs::{CWD, FileType, Mode, makedev, mknodat};
use rustix::io::Errno;
use rustix::mount::{MountFlags, mount, mount_bind, mount_remount};
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

const FROM_ENV: [&str; 5] = ["usr", "bin", "lib", "lib64", "etc"];

/// Name, major and minor number, from
/// <https://www.kernel.org/doc/Documentation/admin-guide/devices.txt>.
const DEVICES: [(&str, u32, u32); 6] = [
    ("null", 1, 3),
    ("zero", 1, 5),
    ("full", 1, 7),
    ("random", 1, 8),
    ("urandom", 1, 9),
    ("tty", 5, 0),
];

/// Builds the view at `sandbox` from the environment at `env_root` and the input disk
/// `inputs`.
pub fn build(env_root: &Path, sandbox: &Path, inputs: &Path, out_bytes: u64) -> io::Result<()> {
    let (ro, no_suid_dev) = (MountFlags::RDONLY, MountFlags::NOSUID | MountFlags::NODEV);
    fs::create_dir_all(sandbox)?;
    tmpfs(sandbox, MountFlags::empty(), "mode=0755")?;
    for name in FROM_ENV {
        let (from, to) = (env_root.join(name), sandbox.join(name));
        match fs::symlink_metadata(&from) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
            Ok(m) if m.is_symlink() => symlink(fs::read_link(&from)?, &to)?,
            Ok(_) => {
                fs::create_dir(&to)?;
                mount_bind(&from, &to).map_err(|e| named(e, "bind", &to))?;
                // A bind ignores MS_RDONLY; only a remount makes it read-only.
                mount_remount(&to, MountFlags::BIND | ro | no_suid_dev, "")
                    .map_err(|e| named(e, "remount read-only", &to))?;
            }
        }
    }
    let dir = |name: &str| -> io::Result<PathBuf> {
        let p = sandbox.join(name);
        fs::create_dir(&p)?;
        Ok(p)
    };
    mount_fs(inputs, &dir("in")?, "ext4", ro | no_suid_dev, "")?;
    let out = format!("uid={PAYLOAD_UID},gid={PAYLOAD_GID},mode=0755,size={out_bytes}");
    tmpfs(&dir("out")?, no_suid_dev, &out)?;
    let proc_flags = no_suid_dev | MountFlags::NOEXEC;
    mount_fs("proc", &dir("proc")?, "proc", proc_flags, "hidepid=2")?;
    let dev = dir("dev")?;
    tmpfs(&dev, MountFlags::NOSUID | MountFlags::NOEXEC, "mode=0755")?;
    for (name, major, minor) in DEVICES {
        let node = dev.join(name);
        let (kind, mode) = (FileType::CharacterDevice, Mode::from_raw_mode(0o666));
        mknodat(CWD, &node, kind, mode, makedev(major, minor))
            .map_err(|e| named(e, "mknod", &node))?;
        // mknod applies the umask.
        fs::set_permissions(&node, fs::Permissions::from_mode(0o666))?;
    }
    tmpfs(&dir("tmp")?, no_suid_dev, "mode=1777")?;
    mount_remount(sandbox, ro, "").map_err(|e| named(e, "remount read-only", sandbox))
}

fn tmpfs(at: &Path, flags: MountFlags, data: &str) -> io::Result<()> {
    mount_fs("tmpfs", at, "tmpfs", flags, data)
}

/// Mounts a file system of type `fs_type` from `source` on `at`.
pub(crate) fn mount_fs(
    source: impl AsRef<Path>,
    at: &Path,
    fs_type: &str,
    flags: MountFlags,
    data: &str,
) -> io::Result<()> {
    let data = CString::new(data)?;
    mount(source.as_ref(), at, fs_type, flags, data.as_c_str())
        .map_err(|e| named(e, &format!("mount {fs_type}"), at))
}

/// An error that names the step and the path, for the guest console and the host.
fn named(e: Errno, what: &str, at: &Path) -> io::Error {
    let e = io::Error::from(e);
    io::Error::new(e.kind(), format!("{what} {}: {e}", at.display()))
}
