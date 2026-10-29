#[test]
fn version_prints_crate_version() {
    assert_cmd::Command::cargo_bin("ignorarium")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicates::str::contains(env!("CARGO_PKG_VERSION")));
}
