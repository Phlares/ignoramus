# 0037 The test check counts test runs

Date: 2026-11-22. Decided and built.

A `cargo test` call can exit 0 when a test binary ended before it printed its result. The exit code then says "pass", and the tests of that binary leave no result.

Terms used here:

- A **test run** is one start of the test harness: a test binary, or one group of doc-tests. It prints `running N tests` (`running 1 test` for one) when it starts, and one line that starts with `test result:` when it finishes its tests.
- A **`cargo test` call** is the whole command. It holds many test runs.
- `gate` is the check runner of `Phlares/workflows`, pinned at v1.11.0 (`CONTRIBUTING.md`, step 4). It is not the power gate of 0013 and 0021.

Decision:

- **One list.** `gate.toml` holds the checks of this repo. CI and a contributor's PC run that list through `gate`. CI runs `lint` in one job and `test` in two, one per toolchain. A PC may add checks of its own in `gate.local.toml`, which git ignores.
- **The pair.** The `test` check runs `cargo test --locked` and counts two kinds of line: the `running` lines and the `test result:` lines. It fails when cargo exits with a code other than 0, when the two counts differ, or when both are 0. The counts are totals over the whole call. More `running` lines than `test result:` lines means that a test run started and did not report.
- **Not cargo's target lines.** The pair does not count cargo's `Running` and `Doc-tests` lines. One `Doc-tests` line can stand for two test runs.
- **The result line.** A `gate` run that reaches its checks ends with one line that starts with `gate: PASS` or `gate: FAIL`. Its `test` part holds both counts, as in `test ok 101=101`. A pull request quotes the whole line.

Why:

- One list, because two lists drift apart. Before this record `CONTRIBUTING.md` named `pre-commit run --all-files`, and CI ran that command with `--hook-stage manual`, which adds the `gitleaks-history` hook.
- The exit code is not enough. In the exit trial (Evidence) a test binary ended with exit code 0 after its `running` line, and the `cargo test` call exited 0. The pair read 102 against 101, and the check failed.
- Langfuse states the rule for the cases of an experiment: "A missing case is not a passing case" (Sources). We apply it to a test run without a `test result:` line.
- A check that fails a passing call is a false alert. In an interview study of 18 machine-learning engineers, false-positive alerts were the most commonly discussed pain point, and a surplus of them led to silenced alerts (Sources). We expect the same of a CI check. So the pair must hold on every passing call under the toolchains CI runs. The doc-test trial (Evidence) rules out cargo's target lines.
- The counts in the result line come from the output. The alternative, a test count typed into the pull request, can be wrong or stale.

Case against:

- Cargo's target lines were the first choice. They count what cargo started, so a target that starts and prints nothing would show. We rejected them after the doc-test trial: a passing call printed 101 target lines and 102 `test result:` lines. From the 2024 edition rustdoc combines compatible doc-tests into one binary and leaves a `compile_fail` test out of it (Sources). The trial showed one test run for the combined group and one for the `compile_fail` test. Every crate here is edition 2024.
- The exit code already fails a `cargo test` call in which a test fails. With exit code 0 the pair fails when the counts differ or both are 0. The cases we expect are two: a test run that starts and does not report, and a call that prints no test run at all. Neither case is known in this repo.
- The counts invite a wrong reading. A reader can take `101=101` as proof that every test ran, so the result line can make a pull request look better tested than it is (Limits).
- The pair matches text that the toolchain prints. A toolchain that changes either line fails the check on a passing call until the pair changes, and the `test (stable)` job moves to a new toolchain with no commit here. We accept that one case: it fails every pull request from that day on, whatever the pull request changes.

Limits:

- The pair counts test runs, not tests. In most cases a deleted or ignored test changes neither count. 10 of the 101 test runs hold no test. Both counts fall together when a whole test target goes, or when a crate's doc-tests go from two test runs to one (the doc-test trial in reverse).
- The pair does not see a test binary that exits 0 before it prints its `running` line.

Evidence:

- **The exit trial.** One added test file held two tests: one calls `std::process::exit(0)`, one panics. Under `gate`, with rustc 1.99.1, that binary printed `running 2 tests` and nothing more. The other test runs went on. The result line read `gate: FAIL`, with `test FAIL 102!=101`: no exit code is named, so cargo exited 0. With `--test-threads=1` the same binary printed `running 2 tests` and the start of the first test's line, and cargo exited 0.
- **The doc-test trial.** One ordinary doc-test was added beside the `compile_fail` doc-test in `crates/deposit/src/zenodo.rs`. `cargo test --locked`, with rustc 1.99.1, printed 101 target lines, 102 `running` lines and 102 `test result:` lines, and exited 0. With rustc 1.89.0, `cargo test --locked --doc -p deposit` printed one `Doc-tests` line, two `running` lines and two `test result:` lines, and exited 0. The trial was not run with rustc 1.100.0.
- Neither trial change is in the tree.

What failure this prevents: a `cargo test` call that exits 0 after a test binary ended early, so that the tests of that binary did not report. Expected, not observed: the exit trial produced it on purpose, and no case is known in this repo.

Constants:

- `count` = `['^running \d+ tests?$', '^test result: ']` (`gate.toml`, check `test`). Basis: the two lines one test run prints. A full call printed 101 of each in the CI run of #133, under rustc 1.89.0 and under rustc 1.100.0. The doc-test trial, run on a PC, printed 102 of each. Rule for change: when a toolchain changes either line. The new pair must hold on a full passing call and on the doc-test trial, and must fail on the exit trial. This record then gets an Amended line.

Sources:

- The Rust Programming Language, "How to Write Tests": the output holds the line `running 1 test`, and "The overall summary `test result: ok.` means that all the tests passed". https://doc.rust-lang.org/book/ch11-01-writing-tests.html
- The Rust Edition Guide, "Rustdoc combined tests": "Starting with the 2024 Edition, rustdoc will attempt to combine documentation tests into a single binary"; a test that uses the `compile_fail` tag "will not be combined with others". https://doc.rust-lang.org/edition-guide/rust-2024/rustdoc-doctests.html
- The rustdoc book, "Documentation tests": "Starting in the 2024 edition, compatible doctests are merged as one before being run." https://doc.rust-lang.org/rustdoc/write-documentation/documentation-tests.html
- Langfuse, "Compare experiments": "A missing case is not a passing case." https://langfuse.com/docs/evaluation/experiments/compare-experiments.md
- Shankar, Garcia, Hellerstein and Parameswaran, "Operationalizing Machine Learning: An Interview Study" (18 engineers): "The most commonly discussed pain point was false-positive alerts", and "A surplus of false-positive alerts led to fatigue and silencing of alerts, which could miss actual performance drops." https://ar5iv.labs.arxiv.org/html/2209.09125
