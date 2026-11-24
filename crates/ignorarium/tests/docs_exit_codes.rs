//! `docs/exit-codes.md` against the table in `src/cli/exit.rs`.
use assert_cmd::Command;
use ignorarium::cli::exit::Code;

const PAGE: &str = "../../docs/exit-codes.md";

#[test]
fn the_exit_code_page_is_up_to_date() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("exit-codes.md");
    Command::cargo_bin("ignorarium")
        .unwrap()
        .env("IGNORARIUM_HOME", tmp.path().join("home"))
        .args(["dev", "gen-exit-codes", "--out"])
        .arg(&out)
        .assert()
        .success();
    assert!(
        std::fs::read(&out).unwrap() == std::fs::read(PAGE).unwrap(),
        "run `ignorarium dev gen-exit-codes` and commit docs/exit-codes.md"
    );
}

#[test]
fn the_page_has_one_row_per_code() {
    let page = std::fs::read_to_string(PAGE).unwrap();
    for c in Code::ALL {
        let row = format!("| {} | {} | {} |", c.number(), c.name(), c.meaning());
        assert!(page.lines().any(|l| l == row), "{row}");
    }
}
