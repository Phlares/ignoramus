# 0027 The litmus

The `litmus` crate writes a small synthetic study whose verdicts are known in advance. The box runs it like any study. One arm must pass and two must not, all in the same run. A box that gives other verdicts is not fit to judge a real study.

Decision:

- **Arms.** Four conditions. `ref` is exactly 0. `good` has mean −0.6 and must pass. `bad` has mean +0.3 and must fail. `edge` has mean 0.0, on the margin, the edge of the null, and must fail. Each value other than `ref` is the arm's mean plus noise uniform within 0.2 of it, drawn from a SHA-256 hash of the cluster's bytes, the unit and the condition. Each claim is "mean below 0" on the range [−1, 1], with `paired = "difference"` against `ref`.
- **Two studies.** `litmus gen` writes an exploratory pilot of 8 clusters and a confirmatory study. The confirmatory study borrows the pilot's sd and icc (`estimates = { from = ... }`), runs a first batch, and re-sizes once from a pinned reserve (0020). Its three hypotheses share the level as 0.34, 0.33 and 0.33.
- **One run shows both.** A run too small to pass anything also "fails" `bad` and `edge`. So a failing bad arm alone shows nothing. The end-to-end test asserts all three verdicts from one run, and the `resize` record, and a `release` leaf that frees the reserve clusters the study did not append.
- **The payload is the litmus.** `gen` copies the running `litmus` program to `payload/run`. With `--condition`, `--unit` and `--repeat`, it reads the unit from `IGNORARIUM_IN` and writes `{"y": v}` to `IGNORARIUM_OUT/values.json`, as any payload does.
- **Expected verdicts.** `gen` writes `expected.json`: `{"good":"pass","bad":"fail","edge":"fail"}`. These are the box's own verdict strings (`pass`, `fail`, `none`). `litmus verify` compares them with the JSON of `ignorarium report`. The end-to-end test builds that JSON from the verdict records, so it shows that `expected.json` uses the box's strings and that `verify` reads that shape; it does not run `ignorarium report`.
- **Replay and freshness.** The seed is written into every data file. The same seed writes the same bytes, the payload aside. Two seeds share no data file hash, so a new seed is fresh data to the plan log.
- **Test log.** Both specs set `test_log = true`. `check` and `register` read the plan log for earlier use of data, and a confirmatory study takes a stream slot. On the test log, the litmus spends neither real data nor a real stream slot.
- **Environment.** Without `--environment`, the litmus spec declares a deviation from the pilot's environment (0017): a pilot on the host names no image, so no pin can match it. Level 2 refuses an empty environment pin at `run`, so a level 2 litmus needs `--environment` with the store digest of an imported image. The payload must then run inside that image, so it should be a static binary.
- **Soak.** `--work-ms` and `--mem-mib` make each row burn CPU time and hold memory. A long soak run needs no new code, only larger counts and these flags.

Default counts: a pilot of 8 clusters, a first batch of 12, a reserve of 60, 2 units per cluster.

The power gate (E-CNF-002) accepts these counts. With the pilot of seed `e2e`, a first batch of 12 needs a reserve of at least 30:

| First | Reserve | Gate |
|---|---|---|
| 12 | 60 | accepts |
| 12 | 30 | accepts |
| 12 | 29 | refuses all three hypotheses: "reaches power 0.00 of 0.80" |
| 12 | 20 | refuses all three: power 0.00 |
| 8 | 34 | refuses `edge`: power 0.00 |
| 8 | 30 | refuses all three: power 0.00 |

At a first batch of 12, the gate jumps from power 0.00 to a pass between a reserve of 29 and 30. The runner's re-size below needs 32 clusters in all (12 + 20 appended), yet the gate refuses 12 + 29 = 41 and first accepts 12 + 30 = 42. Its table rounds the count up (0020), so the gate asks for more clusters than the runner appends. The default keeps a reserve of 60, twice the smallest that passes at seed `e2e`, since another seed gives the pilot another sd. Reserve clusters the study does not append are released, so a large reserve costs only files. The sweep below found no seed that needs more.

Measured. Seed `e2e`, level 0, debug build, one run of `crates/litmus/tests/e2e.rs`:

- The pilot gives a pooled sd of 0.138, 0.118 and 0.112 for `good`, `bad` and `edge`, and an icc of 0, 0.131 and 0.
- The test stream gives slot 1 at level 0.005.
- The re-size reads first 12, reserve 60, needed 32, and appends 20. The release frees the other 40.
- `good` passes with e-value 100,370 at level 0.0017. `bad` fails with e-value 1.0 at level 0.00165. `edge` fails with e-value 0.986 at level 0.00165.
- The test takes 10 s: 64 pilot rows and 256 litmus rows. Before `sha2` and `curve25519-dalek` were compiled with optimization in debug builds, it took 146 s. The runner hashes the whole payload folder before every row, and a debug `litmus` is 14 MB.
- Three changes each turn the test red, on the verdict they touch: `good` at mean +0.3 fails (e-value 1.0); `bad` at mean −0.6 passes (e-value 236,387); `edge` moved to mean −0.6 in the payload passes (e-value 69,071).

Sweep. A server run uses a fresh seed each time, so the counts must hold for any seed, not only `e2e`. The ignored test `seeds_sweep` runs the whole flow for seeds `sweep-0` to `sweep-(N-1)`, each in a new box home: `LITMUS_SWEEP=100 cargo test -p litmus --test e2e -- --ignored seeds_sweep`. It asserts that every seed registers, that `good` passes and `bad` fails in every seed, and that `needed` is at most first + reserve. It allows at most one `edge` pass, since a pass there is a false positive of the test. One run, N = 100, level 0, debug build, 967 s:

| Measure | Result |
|---|---|
| Gate accepts | 100 of 100 |
| `needed` (first + appended) | min 28, median 32, max 35, of 12 + 60 = 72 on hand |
| `appended` | min 16, median 20, max 23 |
| Reserve clusters released | 37 to 44 |
| `good` | pass 100; e-value 13,855 to 314,127, against 1 / 0.0017 = 588 |
| `bad` | fail 100; e-value 1.0 in each |
| `edge` | fail 100; e-value 0.91 to 1.31, against 1 / 0.00165 = 606 |

The default counts stay at pilot 8, first 12, reserve 60, 2 units: every seed passed the gate, and the largest `needed` used 23 of the 60 reserve clusters.

What it does not show:

- The sweep ran at level 0, on one machine, over 100 seeds. A seed whose pilot spread is rarer than 1 in 100 could still need more than the reserve; the gate would then refuse it at `register`, before any litmus row runs.
- At level 0.00165 per seed, 100 seeds expect 0.165 `edge` passes. The sweep cannot measure that rate; it only shows no excess.

- It shows that the box tells a clearly true claim from a clearly false one, on one synthetic design. It does not show the error rate at the margin; the null simulations of 0020 and 0022 test that.
- It runs the end-to-end test at level 0 only. Levels 1 and 2 need a host that has them.
- The payload is trivial. It says nothing about a real payload's faults, its run time, or a run of hours.

Why:

- Build and debug the pipeline on pilot or simulated data, then freeze it before the confirmatory test (PLOS Biology).
- Validate the measurement before a confirmatory test, on items with known answers and on planted errors. While a method is in development, "negative findings may reflect defects in the measurement tools" (Kimmelman, Mogil and Dirnagl).
- A no-worse-than result "could mean either that both drugs are effective or that both drugs are not effective" (FDA). A study that cannot fail a bad arm shows nothing, so the litmus holds a bad arm and an arm on the margin.
- Compare a version with itself before trusting comparisons, and again when the environment changes (A/A tests in cloud benchmarks, Chalmers).
- Fixed checks that tell good from bad, run on a schedule, catch silent degradation (Anthropic postmortem).
- A cheap first tier keeps retries off the costly tier (arXiv 2606.30111). This is one system's design choice, not a measured comparison.
- Artifact review takes "very few hours", and more time hints at a problem (ACM SIGIR artifact badging, a secondary source). The litmus runs in about 10 s at level 0 in a debug build. Its times at levels 1 and 2 are not yet measured.

Case against:

- Models can "fit the synthetic data distributions well", yet synthetic data "are inherently different" from real errors (arXiv 2104.04302). A litmus pass says nothing about a real payload or a run of hours.
- The bad arms sit beside a real study, not inside it. The FDA point is strongest with the weak arm in the same run as the real one.
- Three of the supporting sources are analogies from cloud benchmarks and model monitoring, not from experiment design.
- No source gives a litmus size or schedule. The ten-minute target is the maintainer's.
- The test's seed is fixed, so it sees one draw of `edge`. Over many seeds `edge` passes at most at the test's level. One seed cannot show that rate.
- The gate's counts depend on the pilot's sd, which changes with the seed. The sweep covers 100 seeds; a rarer seed may need a reserve larger than 60.

Sources:

- PLOS Biology, on building and debugging the pipeline on pilot or simulated data, https://journals.plos.org/plosbiology/article?id=10.1371/journal.pbio.3000246
- Kimmelman, Mogil and Dirnagl (2014), PLOS Biology, https://journals.plos.org/plosbiology/article?id=10.1371/journal.pbio.1001863
- FDA, "Non-Inferiority Clinical Trials to Establish Effectiveness", https://www.fda.gov/media/133660/download
- Chalmers, A/A tests in cloud benchmarks, https://research.chalmers.se/publication/511491/file/511491_Fulltext.pdf
- Anthropic, "A postmortem of three recent issues", https://www.anthropic.com/engineering/a-postmortem-of-three-recent-issues
- arXiv 2606.30111, https://arxiv.org/html/2606.30111v3
- ACM SIGIR artifact badging, https://sigir.org/general-information/acm-sigir-artifact-badging/
- arXiv 2104.04302, https://ar5iv.labs.arxiv.org/html/2104.04302
