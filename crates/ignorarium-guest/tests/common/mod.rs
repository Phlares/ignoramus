use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Writes an executable through a child process, so no write fd leaks into a fork (ETXTBSY).
pub fn write_exec(path: &Path, text: &str) {
    let mut child = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
}
