#[test]
fn version_prints_crate_version() {
    assert_cmd::Command::cargo_bin("ignorarium")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicates::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn the_command_line_codes() {
    let dir = tempfile::tempdir().unwrap();
    let code = |args: &[&str]| {
        let mut c = assert_cmd::Command::cargo_bin("ignorarium").unwrap();
        c.env("IGNORARIUM_HOME", dir.path()).args(args);
        c.assert().get_output().status.code()
    };
    assert_eq!(code(&["--help"]), Some(0));
    assert_eq!(code(&["--version"]), Some(0));
    assert_eq!(code(&[]), Some(2));
    assert_eq!(code(&["guide", "--nope"]), Some(2));
    assert_eq!(code(&["check"]), Some(2));
}
