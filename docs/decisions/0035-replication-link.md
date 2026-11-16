# 0035 A replication names its original and one criterion

Date: 2026-11-15. Decided and built.

A re-run of a study could not show that it replicates the earlier one. Nothing in its spec or its records named the original, the result it repeats, or the rule that decides success.

Decision:

- **The spec.** A replication is a new study whose spec holds:

  ```toml
  [study.replicates]
  study = "<the original's id>"
  done = "<the hash of the original's done record>"
  hypothesis = "<a hypothesis id, the same in both studies>"
  criterion = "verdict"
  ```

  `done` is a bare record hash: 64 lowercase hex characters, with no prefix (`E-FMT-003`). `criterion` takes one value, `verdict`.
- **`check`.**
  - The original is not registered here, or its log holds no `done`: the link is pending (`W-REF-001`), and `check` exits 3.
  - The spec names its own id: `E-REF-002`.
  - `done` is not the hash of the original's `done` record: `E-RPL-001`.
  - The original's verdict for the hypothesis is not `pass` on basis `test`: `E-RPL-002`.
  - The original is in the test log and this study is not: `E-RPL-002`. An original in the test log backs only a replication in the test log. A study in the test log never counts.
  - This study is not confirmatory, or has no hypothesis with a claim under that id: `E-RPL-003`.
- **`register`** refuses a pending or failing link. It writes `replicates` into the register record: the four spec values and the hash of the original's verdict record for the hypothesis.
- **The criterion.** `verdict` is the two-trials rule. The replication succeeds when its own registered test of the hypothesis passes, at its own level, given that the original's test passed. It needs no new setting.
- **The result.** No new record. The box reads the replication's own verdict record for the hypothesis. The state is `met` for `pass` and `not met` for `fail`. With no verdict it is `no result` once the log holds `done`, else `pending`. A stopped study has no card, so `no result` and `pending` appear in `status` only. `status` prints the pair and the state for every replication.
- **The card.** A replication's Markdown card prints the pair: the original's id, its `done` hash, the hypothesis, the original's verdict, the criterion and its result. It also says whether the two studies hold the same estimate keys for the hypothesis (0014) and registered the same payload. A difference is printed, not refused. The original's card does not change.
- **The pin.** `study.replicates.done` is a record hash typed in a spec. It is the one exception to 0014: a pin that the box checks against the original's own signed log, as it checks `pins.payload` against the payload. So a plan cannot name an end the original does not have.
- **No label.** The link does not change the label a study can earn. The value `replicated` of `study.label` stays as it was.

Why:

- On the same 112 effects, replication success ran from 25% to 63% depending on the criterion (eLife 71601). So the spec fixes one criterion before the replication runs.
- A same-direction criterion scores 50% on noise, and a prediction-interval criterion gets easier to pass as the original gets noisier (eLife 71601). Neither is offered.
- The two-trials rule needs both studies to pass. It is strict and easy to explain (arXiv 2204.06960). It reuses the verdict and level each study already has.
- Two results that both fail to pass were counted as a successful replication in one project, and that count does not stand under an equivalence re-analysis (arXiv 2204.06960). So the box refuses an original that did not pass.
- The rule needs two tests. An exploratory claim passes on its point estimate (basis `estimate`), which is not a test. So only a confirmatory original's test pass can be replicated. A confirmatory study of an exploratory signal is an ordinary confirmatory study.

Case against:

- Every source for the criterion uses p-values. None covers the two-trials rule with e-values, which this box tests with.
- The sceptical test has more power than the two-trials rule (arXiv 2204.06960). A true effect replicates less often under the stricter rule. We rejected the sceptical test because its false-success rate, given the original, reaches 7.5 to 8%.
- One criterion only. A reader who wants another criterion computes it from the records.
- The original must hold `done`. An original finished by an older box needs one more run before anything can replicate it.
- The pin breaks 0014's rule that an author never types a record hash. The box could read the original's `done` itself. We keep the pin because it fixes which end the plan was written against.

Limits:

- The TeX card has no replication block.
- `no result` and `pending` show in `status` only.
- The box does not size a replication. The power gate is unchanged.
- A different payload or estimate key is printed, not refused.

What failure this prevents: a re-run with nothing to show that it replicates an earlier study, and a "replication" of a result that never passed a test. Expected, not observed.

Constants: none. The record sets no value. The criterion's level is each study's own.

Sources:

- eLife 71601, on replication criteria: on the same 112 effects, replication success ran from 25% to 63% by criterion; a same-direction criterion scores 50% on noise; prediction-interval criteria get easier as the original gets noisier; so fix one criterion before the replication runs. https://elifesciences.org/articles/71601
- arXiv 2204.06960, the two-trials rule and the sceptical alternative: the two-trials rule needs both studies to pass and is strict and easy to explain; the sceptical alternative has more power but a conditional false-success rate up to 7.5 to 8%; two results that both fail to pass were counted as a successful replication in one project and do not stand under an equivalence re-analysis. https://arxiv.org/html/2204.06960v3
