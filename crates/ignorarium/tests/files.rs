use ignorarium::files::{WriteOnceError, Written, write_once};
use std::sync::Barrier;

fn names(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn create_then_same_then_differs_leave_no_temp() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("f");
    assert_eq!(write_once(&p, b"a", 0o600).unwrap(), Written::Created);
    assert_eq!(names(d.path()), ["f"]);
    assert_eq!(write_once(&p, b"a", 0o600).unwrap(), Written::Same);
    assert!(matches!(
        write_once(&p, b"b", 0o600),
        Err(WriteOnceError::Differs)
    ));
    assert_eq!(names(d.path()), ["f"]);
    assert_eq!(std::fs::read(&p).unwrap(), b"a");
}

#[test]
fn missing_directory_is_an_io_error() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("nope").join("f");
    assert!(matches!(
        write_once(&p, b"a", 0o600),
        Err(WriteOnceError::Io(_))
    ));
}

#[cfg(unix)]
#[test]
fn mode_is_applied() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("f");
    write_once(&p, b"a", 0o600).unwrap();
    let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[cfg(unix)]
#[test]
fn planted_symlink_is_refused_and_target_untouched() {
    let d = tempfile::tempdir().unwrap();
    let t = d.path().join("target");
    std::fs::write(&t, b"x").unwrap();
    let p = d.path().join("f");
    std::os::unix::fs::symlink(&t, &p).unwrap();
    assert!(matches!(
        write_once(&p, b"a", 0o600),
        Err(WriteOnceError::Link)
    ));
    assert_eq!(std::fs::read(&t).unwrap(), b"x");
    assert_eq!(names(d.path()), ["f", "target"]);
}

#[cfg(unix)]
#[test]
fn dangling_symlink_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("f");
    std::os::unix::fs::symlink(d.path().join("gone"), &p).unwrap();
    assert!(matches!(
        write_once(&p, b"a", 0o600),
        Err(WriteOnceError::Link)
    ));
    assert!(!d.path().join("gone").exists());
}

#[cfg(unix)]
#[test]
fn stale_temp_lookalikes_do_not_matter() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("f");
    let t = d.path().join("elsewhere");
    std::fs::write(&t, b"x").unwrap();
    std::fs::write(d.path().join(".f.tmp"), b"old").unwrap();
    std::os::unix::fs::symlink(&t, d.path().join(".f.1.00.tmp")).unwrap();
    assert_eq!(write_once(&p, b"a", 0o600).unwrap(), Written::Created);
    assert_eq!(std::fs::read(&t).unwrap(), b"x");
    assert_eq!(std::fs::read(&p).unwrap(), b"a");
}

type Outcome = Result<Written, WriteOnceError>;

fn race(a: &'static [u8], b: &'static [u8]) -> (Vec<Outcome>, Vec<u8>) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("f");
    let bar = Barrier::new(2);
    let res = std::thread::scope(|s| {
        let hs: Vec<_> = [a, b]
            .into_iter()
            .map(|bytes| {
                let (p, bar) = (&p, &bar);
                s.spawn(move || {
                    bar.wait();
                    write_once(p, bytes, 0o600)
                })
            })
            .collect();
        hs.into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(names(d.path()), ["f"]);
    (res, std::fs::read(&p).unwrap())
}

#[test]
fn racing_different_bytes_one_wins() {
    let inputs: [&[u8]; 2] = [b"one", b"two"];
    for _ in 0..50 {
        let (res, data) = race(b"one", b"two");
        let created: Vec<usize> = (0..2)
            .filter(|&i| matches!(res[i], Ok(Written::Created)))
            .collect();
        assert_eq!(created.len(), 1);
        assert!(matches!(res[1 - created[0]], Err(WriteOnceError::Differs)));
        assert_eq!(data, inputs[created[0]]);
    }
}

#[test]
fn racing_same_bytes_one_created_one_same() {
    for _ in 0..50 {
        let (res, data) = race(b"same", b"same");
        let created = res
            .iter()
            .filter(|r| matches!(r, Ok(Written::Created)))
            .count();
        let same = res
            .iter()
            .filter(|r| matches!(r, Ok(Written::Same)))
            .count();
        assert_eq!((created, same), (1, 1));
        assert_eq!(data, b"same");
    }
}

#[test]
fn write_once_errors_display_their_cause() {
    assert_eq!(
        WriteOnceError::Differs.to_string(),
        "file exists and differs"
    );
    assert_eq!(WriteOnceError::Link.to_string(), "path is a symbolic link");
    let e = WriteOnceError::from(std::io::Error::other("boom"));
    assert!(matches!(e, WriteOnceError::Io(_)));
    assert_eq!(e.to_string(), "I/O failed: boom");
}

#[cfg(unix)]
#[test]
fn open_log_names_a_symbolic_link() {
    let d = tempfile::tempdir().unwrap();
    let real = d.path().join("real");
    std::fs::write(&real, "").unwrap();
    let link = d.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let e = ignorarium::files::open_log(&link).unwrap_err();
    assert_eq!(e.to_string(), "log path is a symbolic link");
}
