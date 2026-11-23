# 0037 The test check counts test runs

Date: 2026-11-22. Decided and built.

A `cargo test` call can exit 0 when a test binary ended before it printed its result. The exit code then says "pass", and the tests of that binary leave no result.

Terms used here:

- A **test run** is one start of the test harness: a test binary, or one group of doc-tests. It prints `running N tests` (`running 1 test` for one) when it starts, and one line that starts with `test result:` when it ends.
- A **`cargo test` call** is the whole command. It holds many test runs.
- `gate` is the check runner of `Phlares/workflows`, pinned at v1.11.0 (`CONTRIBUTING.md`, step 4). It is not the power gate of 0013 and 0021.

Decision:

- **One list.** `gate.toml` holds the checks of this repo. CI and a contributor's PC run that list through `gate`. CI runs each check in its own job. A PC may add checks of its own in `gate.local.toml`, which git ignores.
- **The pair.** The `test` check runs `cargo test --locked` and counts two kinds of line: the `running` lines and the `test result:` lines. It fails when cargo exits with a code other than 0, when the two counts differ, or when both are 0. Equal counts mean that every test run that printed its `running` line also printed its `test result:` line.
- **Not cargo's target lines.** The pair does not count cargo's `Running` and `Doc-tests` lines. One `Doc-tests` line can stand for two test runs.
- **The result line.** `gate` ends with one line that starts with `gate: PASS` or `gate: FAIL`. Its `test` part holds both counts, as in `test ok 101=101`. A pull request quotes the whole line.

Why:

- One list, because two lists drift apart. Before this record `CONTRIBUTING.md` named `pre-commit run --all-files`, and CI ran that command with `--hook-stage manual`, which adds the `gitleaks-history` hook.
- The exit code is not enough. In the exit trial (Evidence) a test binary ended with exit code 0 after its `running` line, and the `cargo test` call exited 0. The pair read 102 against 101, and the check failed.
- Langfuse states the rule for the cases of an experiment: "A missing case is not a passing case" (Sources). We apply it to a test run without a `test result:` line.
- A check that fails a passing call is a false alert. In an interview study of 18 machine-learning engineers, false-positive alerts were the most commonly discussed pain point, and a surplus of them led to silenced alerts (Sources). We expect the same of a CI check. So the pair must hold on every passing call under the toolchains CI uses today, which rules out cargo's target lines (Case against).
- The counts in the result line come from the output. The alternative, a test count typed into the pull request, can be wrong or stale.

Case against:

- Cargo's target lines were the first choice. They count what cargo started, so a target that starts and prints nothing would show. We rejected them after the doc-test trial (Evidence): a passing call printed 101 target lines and 102 `test result:` lines. From the 2024 edition rustdoc combines compatible doc-tests into one binary and leaves a `compile_fail` test out of it (Sources). The trial showed one test run for the combined group and one for the `compile_fail` test. Every crate here is edition 2024.
- The exit code already fails a `cargo test` call in which a test fails. The pair adds one case, an early end with exit code 0. No such case is known in this repo.
- The counts invite a wrong reading. A reader can take `101=101` as proof that every test ran. It shows only what the pair states (Limits).
- The pair matches text that the toolchain prints. A toolchain that changes either line fails the check on a passing call until the pair changes, and the `stable` job moves to a new toolchain with no commit here. We accept that one case: it fails every pull request at once, so nobody can take it for a fault of their change.

Limits:

- The pair counts test runs, not tests. A deleted or ignored test changes neither count. 10 of the 101 test runs hold no test. Both counts fall together only when a whole test target goes.
- The pair does not see a test binary that exits 0 before it prints its `running` line.

Evidence:

- **The exit trial.** One added test file held two tests: one calls `std::process::exit(0)`, one panics. Under `gate`, with rustc 1.99.1, that binary printed `running 2 tests` and nothing more. The other test runs went on. The result line read `gate: FAIL`, with `test FAIL 102!=101`: no exit code is named, so cargo exited 0. With `--test-threads=1` the same binary printed `running 2 tests` and the start of the first test's line, and cargo exited 0.
- **The doc-test trial.** One ordinary doc-test was added beside the `compile_fail` doc-test in `crates/deposit/src/zenodo.rs`. `cargo test --locked`, with rustc 1.99.1, printed 101 target lines, 102 `running` lines and 102 `test result:` lines, and exited 0.
- Neither trial file is in the tree.

What failure this prevents: a `cargo test` call that exits 0 after a test binary ended early, so that the tests of that binary did not report. Produced in the exit trial; no case is known in this repo.

Constants:

- `count` = `['^running \d+ tests?$', '^test result: ']` (`gate.toml`, check `test`). Basis: the two lines one test run prints. On 2026-11-22 a full call printed 101 of each under rustc 1.89.0 and 1.100.0 in CI, and 102 of each in the doc-test trial. Rule for change: when a toolchain changes either line. The new pair must hold on a full passing call and on the doc-test trial, and must fail on the exit trial. This record then gets an Amended line.

Sources:

- The Rust Programming Language, "How to Write Tests": the output holds the line `running 1 test`, and "The overall summary `test result: ok.` means that all the tests passed". https://doc.rust-lang.org/book/ch11-01-writing-tests.html
- The Rust Edition Guide, "Rustdoc combined tests": "Starting with the 2024 Edition, rustdoc will attempt to combine documentation tests into a single binary"; a test that uses the `compile_fail` tag "will not be combined with others". https://doc.rust-lang.org/edition-guide/rust-2024/rustdoc-doctests.html
- The rustdoc book, "Documentation tests": "Starting in the 2024 edition, compatible doctests are merged as one before being run." https://doc.rust-lang.org/rustdoc/write-documentation/documentation-tests.html
- Langfuse, "Compare experiments": "A missing case is not a passing case." https://langfuse.com/docs/evaluation/experiments/compare-experiments.md
- Shankar, Garcia, Hellerstein and Parameswaran, "Operationalizing Machine Learning: An Interview Study" (18 engineers): "The most commonly discussed pain point was false-positive alerts", and "A surplus of false-positive alerts led to fatigue and silencing of alerts, which could miss actual performance drops." https://ar5iv.labs.arxiv.org/html/2209.09125
