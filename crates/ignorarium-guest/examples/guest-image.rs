//! Assembles a guest image: `initramfs.cpio`, `rootfs.ext4` and `image.json`.
//!
//! guest-image --agent <file> --rootfs-dir <dir> --kernel <file> --out <dir>
use ignorarium_guest::image::{KERNEL, check_pin, ext4, initramfs, sha256_file, size_mib};
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: HashMap<String, PathBuf> = HashMap::new();
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next().ok_or(format!("{flag} needs a value"))?;
        args.insert(flag, value.into());
    }
    let mut get = |flag: &str| {
        args.remove(flag)
            .ok_or(format!("missing {flag}; usage: guest-image --agent <file> --rootfs-dir <dir> --kernel <file> --out <dir>"))
    };
    let (agent, rootfs_dir) = (get("--agent")?, get("--rootfs-dir")?);
    let (kernel, out) = (get("--kernel")?, get("--out")?);
    if let Some(extra) = args.keys().next() {
        return Err(format!("unknown flag {extra}").into());
    }
    check_pin(&kernel, &KERNEL)?;
    fs::create_dir_all(&out)?;
    let initramfs_path = out.join("initramfs.cpio");
    let rootfs_path = out.join("rootfs.ext4");
    fs::write(&initramfs_path, initramfs(&fs::read(&agent)?))?;
    ext4(Some(&rootfs_dir), &rootfs_path, size_mib(&rootfs_dir)?)?;
    let image = json!({
        "kernel": { "pin": KERNEL.name, "sha256": sha256_file(&kernel)? },
        "agent": sha256_file(&agent)?,
        "initramfs": sha256_file(&initramfs_path)?,
        "rootfs": sha256_file(&rootfs_path)?,
    });
    fs::write(out.join("image.json"), serde_json::to_vec_pretty(&image)?)?;
    Ok(())
}
