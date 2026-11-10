use litmus::value;
use std::fs;
use std::process::{Command, Output};

const FILE: &[u8] = b"a first 0001 0\na first 0001 1\n";

struct Lab {
    dir: tempfile::TempDir,
}

impl Lab {
    fn new() -> Lab {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("in/data")).unwrap();
        fs::create_dir_all(dir.path().join("out")).unwrap();
        fs::write(dir.path().join("in/data/first-0001.txt"), FILE).unwrap();
        Lab { dir }
    }
    fn out(&self) -> std::path::PathBuf {
        self.dir.path().join("out")
    }
    fn command(&self, condition: &str, unit: &str) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_litmus"));
        c.args(["--condition", condition, "--unit", unit, "--repeat", "0"])
            .current_dir(self.dir.path().join("in"))
            .env_clear()
            .env("IGNORARIUM_IN", self.dir.path().join("in"))
            .env("IGNORARIUM_OUT", self.out())
            .env("IGNORARIUM_PARAMS", "{}");
        c
    }
    fn run(&self, condition: &str, unit: &str) -> Output {
        self.command(condition, unit).output().unwrap()
    }
    fn values(&self) -> Option<String> {
        fs::read_to_string(self.out().join("values.json")).ok()
    }
    fn assert_fails(&self, out: &Output) {
        assert!(!out.status.success(), "{out:?}");
        assert!(!out.stderr.is_empty());
        assert_eq!(self.values(), None);
        assert!(!self.dir.path().join("in/values.json").exists());
    }
}

#[test]
fn writes_values_json() {
    let lab = Lab::new();
    let out = lab.run("good", "data/first-0001.txt#1");
    assert!(out.status.success(), "{out:?}");
    let v = value(FILE, 1, "good").unwrap();
    let got: serde_json::Value = serde_json::from_str(&lab.values().unwrap()).unwrap();
    assert_eq!(got.as_object().unwrap().len(), 1);
    assert_eq!(got["y"].as_f64().unwrap().to_bits(), v.to_bits());
}

#[test]
fn ref_writes_zero() {
    let lab = Lab::new();
    assert!(lab.run("ref", "data/first-0001.txt#0").status.success());
    let got: serde_json::Value = serde_json::from_str(&lab.values().unwrap()).unwrap();
    assert_eq!(got["y"].as_f64(), Some(0.0));
}

#[test]
fn unknown_condition_fails() {
    let lab = Lab::new();
    lab.assert_fails(&lab.run("nope", "data/first-0001.txt#0"));
}

#[test]
fn unit_past_end_fails() {
    let lab = Lab::new();
    lab.assert_fails(&lab.run("good", "data/first-0001.txt#2"));
    lab.assert_fails(&lab.run("good", "data/first-0001.txt#4000000000"));
}

#[test]
fn blank_line_is_not_a_unit() {
    let lab = Lab::new();
    fs::write(lab.dir.path().join("in/data/blank.txt"), "a\n\n").unwrap();
    lab.assert_fails(&lab.run("good", "data/blank.txt#1"));
}

#[test]
fn malformed_unit_fails() {
    let lab = Lab::new();
    for unit in [
        "data/first-0001.txt",
        "data/first-0001.txt#",
        "data/first-0001.txt#x",
        "data/first-0001.txt#-1",
        "#0",
        "data/missing.txt#0",
    ] {
        lab.assert_fails(&lab.run("good", unit));
    }
}

#[test]
fn unit_outside_in_dir_fails() {
    let lab = Lab::new();
    fs::write(lab.dir.path().join("secret.txt"), "x\n").unwrap();
    lab.assert_fails(&lab.run("good", "../secret.txt#0"));
    let abs = format!("{}#0", lab.dir.path().join("secret.txt").display());
    lab.assert_fails(&lab.run("good", &abs));
}

#[test]
fn missing_in_dir_fails() {
    let lab = Lab::new();
    let mut c = lab.command("good", "data/first-0001.txt#0");
    c.env_remove("IGNORARIUM_IN");
    lab.assert_fails(&c.output().unwrap());
}

#[test]
fn missing_out_dir_fails() {
    let lab = Lab::new();
    let mut c = lab.command("good", "data/first-0001.txt#0");
    c.env_remove("IGNORARIUM_OUT");
    lab.assert_fails(&c.output().unwrap());
    let gone = lab.dir.path().join("gone");
    let out = lab
        .command("good", "data/first-0001.txt#0")
        .env("IGNORARIUM_OUT", &gone)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!gone.exists());
}

#[test]
fn bad_params_fail() {
    let lab = Lab::new();
    for params in ["", "{", "[]", r#"{"work_ms":"x"}"#, r#"{"mem_mib":-1}"#] {
        let out = lab
            .command("good", "data/first-0001.txt#0")
            .env("IGNORARIUM_PARAMS", params)
            .output()
            .unwrap();
        lab.assert_fails(&out);
    }
}

#[test]
fn unknown_flag_fails() {
    let lab = Lab::new();
    let out = lab
        .command("good", "data/first-0001.txt#0")
        .arg("--bogus")
        .arg("1")
        .output()
        .unwrap();
    lab.assert_fails(&out);
}

#[test]
fn unknown_command_is_named() {
    for args in [&["foo"][..], &["foo", "--condition"]] {
        let out = Command::new(env!("CARGO_BIN_EXE_litmus"))
            .args(args)
            .output()
            .unwrap();
        assert!(!out.status.success());
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.starts_with("litmus: unknown command foo\n"), "{err}");
    }
}

/// Runs the payload with `params` and returns its own (CPU seconds, peak resident KiB).
#[cfg(target_os = "linux")]
// wait4 reaps the child and returns its own usage; Child::wait cannot.
#[allow(clippy::zombie_processes)]
fn usage(lab: &Lab, params: &str) -> (f64, i64) {
    let child = lab
        .command("good", "data/first-0001.txt#0")
        .env("IGNORARIUM_PARAMS", params)
        .spawn()
        .unwrap();
    let mut status = 0;
    // SAFETY: `ru` is plain data that wait4 fills; the pid is our own live child.
    let ru = unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        let pid = libc::wait4(child.id() as i32, &mut status, 0, &mut ru);
        assert_eq!(pid, child.id() as i32);
        ru
    };
    assert_eq!(status, 0);
    let secs = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    (secs(ru.ru_utime) + secs(ru.ru_stime), ru.ru_maxrss)
}

#[cfg(target_os = "linux")]
#[test]
fn work_param_burns_cpu() {
    let lab = Lab::new();
    assert!(usage(&lab, "{}").0 < 0.1);
    let cpu = usage(&lab, r#"{"work_ms":250}"#).0;
    assert!(cpu >= 0.2, "{cpu}");
    assert!(lab.values().is_some());
}

#[cfg(target_os = "linux")]
#[test]
fn mem_param_touches_memory() {
    let lab = Lab::new();
    assert!(usage(&lab, "{}").1 < 32 * 1024);
    assert!(usage(&lab, r#"{"mem_mib":64}"#).1 >= 64 * 1024);
}

#[test]
fn success_leaves_no_temp_file() {
    let lab = Lab::new();
    assert!(lab.run("good", "data/first-0001.txt#0").status.success());
    let mut names: Vec<_> = fs::read_dir(lab.out())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(names, ["values.json"]);
}

#[test]
fn failed_rename_leaves_no_values_file() {
    let lab = Lab::new();
    fs::create_dir(lab.out().join("values.json")).unwrap();
    let out = lab.run("good", "data/first-0001.txt#0");
    assert!(!out.status.success(), "{out:?}");
    assert!(!lab.out().join("values.json").is_file());
    assert!(!lab.out().join("values.json.tmp").exists());
}
