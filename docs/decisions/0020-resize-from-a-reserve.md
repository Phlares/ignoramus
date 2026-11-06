# 0020 Re-size from a reserve

A confirmatory study may declare a first batch, a pinned reserve of fresh clusters, and one blinded re-size. The box runs the first batch, counts the clusters each claimed stratum needs from that batch's own data, then appends that many reserve clusters in a fixed order or stops. At the end it releases the reserve clusters it never measured.

Decision:

- **Declaration.** Inputs with role `reserve` hold the clusters the re-size may add. `[design.resize]` with `rule = "blinded-once"` declares the re-size. Five rules guard them: a reserve needs a declared re-size (E-RSV-001), and a re-size needs a reserve (E-RSV-002); every reserve input has a SHA-256 pin (E-RSV-003); every stratum a claim lists has a reserve, no reserve lies in a stratum no claim lists, and a claim with no strata keeps its reserve in one stratum (E-RSV-004); only a confirmatory study with mean claims re-sizes (E-RSV-005). A reserve input must be fresh data, as a confirmation input must (E-CNF-004, E-CNF-005).
- **One estimate.** When every first-batch row has a signed record, the re-size reads, per claimed stratum, each first-batch cluster's observation for the claim's contrast: the clipped mean the test sees (0013). It takes s = √(mean(x²)), the root mean square about 0, not about the mean. It never computes or stores a mean of x, and it reads no e-value and no outcome other than the claim's.
- **The count.** Per claimed stratum, the box searches the smallest count of clusters whose simulated power (0013) reaches `design.power`^(1/k), k the strata the claim lists, so the joint event reaches `design.power`. The search treats each cluster's observation as normal with sd s about `power_at`, runs 4,000 replicates from the seed `resize/<hypothesis>/<stratum>`, at the study's level × `alpha_share`, and stops at the first batch plus the reserve. Over several claims a stratum takes the largest count. It appends the count less its first batch, or none. A stratum whose reserve cannot reach its count stops the study.
- **The schedule.** Reserve rows come after every confirmation row, ordered by stratum, then by the reserve input's SHA-256 pin, then in the confirmation stage's unit, condition and repeat order. Their stage is `reserve`. A claim with no strata would take its count from the first stratum's reserve before any of the next, which tilts its pooled mean toward that stratum, so E-RSV-004 keeps such a reserve in one stratum. The runner skips a reserve row until the re-size admits it. Determinism rechecks (0018) copy only rows outside the reserve, so a recheck never waits on a row that never runs.
- **The `resize` record.** One signed record, written once, before any reserve row: per stratum `first`, `reserve`, `needed` (a count, or `null` when the reserve cannot reach it) and `appended`, a `decision` of `continue` or `stop`, and a `reason` (`null`, or why the study stops). It holds no sd; the `analysis` record adds `resize_sd` per stratum when the study ends. `status` shows the decision and, per stratum, `first`, `needed` and `appended`, nothing else. A first batch with a failed row cannot give an honest s, so the study stops.
- **Stop.** No further row runs. The analysis covers the first batch; each claim fails with e-value 0 and the reason `stopped: ` followed by the `resize` record's reason: `the reserve cannot reach the power the claim needs`, or `incomplete units in the first batch: N` when N first-batch units have no value; and the study's stream slot closes with 0, as for a claim that is not shown.
- **Order of evidence.** In each stratum the test reads the first batch in its seeded order (0013), then the appended reserve clusters in reserve order.
- **Release.** At the end, `finish` appends one plan-log leaf of kind `release`, labelled with the study id, that lists the exact and normalized hashes of every reserve input the study did not append. The freshness check (E-CNF-005; 0012) then ignores the study's own plan leaf for those data, so a later study may use them. A `release` leaf alone is never a use; appended reserve data stay used.
- **The registration gate.** For a study that re-sizes, the power gate simulates the whole design at the borrowed sd and icc, with units about `power_at`. Per replicate and stratum, it draws the first batch, takes its s, reads the count from a table, stops the replicate when the count exceeds the first batch plus the reserve, and otherwise draws the appended clusters and tests the first batch then the appended ones. A replicate that stops fails. The gate passes when the share of passing replicates reaches `design.power`. The table holds 8 values of s, geometric from s0 / 4 to 4 s0, with s0 = √(sd_c² + `power_at`²) and sd_c the sd of a first-batch cluster's mean (0013). Each value has its count as above. A replicate's s takes the count of the first value at or above it, which rounds the count up; past the top value it stops. The z count floor (0013) is skipped, since it assumes a fixed count of clusters.
- **Limits.** The search for a count tries up to 16,384 clusters per stratum, and one call draws at most 2,000 million values. The numbers in 0013 (1,024 clusters, 100 million values) describe the earlier version.

Measured. A null simulation of the design, in release mode on one 6-CPU cloud host, all 22 scenarios below at once, took 1,915 s (32 minutes), with the bet cap at 3/4 (0022). Each scenario has one stratum, a first batch of 21 clusters of 4 units, a mean claim below ln 1.04 on [−ln 2, ln 2] at level 0.005, and a re-size at `power_at` = ln 1.02 for power 0.894 (0.8^(1/2), one stratum's share of a two-stratum claim at power 0.8). The data are drawn with their mean at the margin, the edge of the null. Each scenario runs 100,000 replicates in 25 batches of 4,000, each batch on its own seed with its own table. A rejection is a pass of the claim. The test passes when every rejection share is at most 0.005 + 3 × √(0.005 × 0.995 / 100,000) = 0.0056691. A crossed replicate is one whose e-value reached 1 / level after some observation, not only the last; by Ville's inequality its share is at most the level too. A replicate that stops is neither passed nor crossed. That share is shown and not tested. Shares are given with their Monte Carlo standard error.

| Reserve | sd | icc | Rejection share (se) | Stop share (se) | Crossed share (se) |
|---|---|---|---|---|---|
| 0 | 0.03 | 0.1 | 0 | 1 | 0 |
| 0 | 0.03 | 0.8 | 0 | 1 | 0 |
| 0 | 0.15 | 0.1 | 0 | 1 | 0 |
| 0 | 0.15 | 0.8 | 0 | 1 | 0 |
| 500 | 0.03 | 0.1 | 0 | 0 | 0 |
| 500 | 0.03 | 0.8 | 0 | 0.00563 (0.00024) | 0 |
| 500 | 0.15 | 0.1 | 0 | 0.93451 (0.00078) | 0.00001 (0.00001) |
| 500 | 0.15 | 0.8 | 0 | 0.99948 (0.00007) | 0 |
| 2,500 | 0.03 | 0.1 | 0 | 0 | 0 |
| 2,500 | 0.03 | 0.8 | 0 | 0 | 0 |
| 2,500 | 0.15 | 0.1 | 0.00006 (0.00002) | 0 | 0.00040 (0.00006) |
| 2,500 | 0.15 | 0.8 | 0.00008 (0.00003) | 0.10136 (0.00095) | 0.00168 (0.00013) |

At level 0.005 the test almost never rejects, so these shares cannot show a fault that raises rejections. Two more scenarios run the same design at level 0.05, with a reserve of 2,500 and an icc of 0.1, against the bound 0.05 + 3 × √(0.05 × 0.95 / 100,000) = 0.0520676. At level 0.05 the sd 0.03 scenario still rejects 0 times in 100,000, so only the sd 0.15 scenario can show a raised error rate; it measured 0.00228 against a bound of 0.0520676:

| Level | Reserve | sd | icc | Rejection share (se) | Stop share (se) | Crossed share (se) |
|---|---|---|---|---|---|---|
| 0.05 | 2,500 | 0.03 | 0.1 | 0 | 0 | 0 |
| 0.05 | 2,500 | 0.15 | 0.1 | 0.00228 (0.00015) | 0.00002 (0.00001) | 0.00702 (0.00026) |

Normal data at the margin never bring a score near the top of the range, where the bet cap acts (0022). Six more scenarios draw each cluster's observation from a two-point null whose mean is the margin m = ln 1.04: the two ends of the range, or the top of the range, ln 2, and m − a (ln 2 − m) for a skew a of 0.03 or 0.3. On this range the top scores 0.893, not 1, since the score divides by the longer side, m + ln 2. Each runs at levels 0.005 and 0.05 with a reserve of 2,500. Its sd is the two points' sd, with an icc of 1, so the re-size table centres on the spread the data have:

| Level | Shape | Points | Chance of the top | sd | Rejection share (se) | Stop share (se) | Crossed share (se) |
|---|---|---|---|---|---|---|---|
| 0.005 | ends, not informative | −0.693147, 0.693147 | 0.52829 | 0.69204 | 0 | 1 | 0 |
| 0.005 | skew 0.03 | 0.019603, 0.693147 | 0.02913 | 0.11326 | 0.00007 (0.00003) | 0.46219 (0.00158) | 0.00018 (0.00004) |
| 0.005 | skew 0.3, not informative | −0.156957, 0.693147 | 0.23077 | 0.35817 | 0 | 1 | 0 |
| 0.05 | ends, not informative | −0.693147, 0.693147 | 0.52829 | 0.69204 | 0 | 1 | 0 |
| 0.05 | skew 0.03 | 0.019603, 0.693147 | 0.02913 | 0.11326 | 0.00300 (0.00017) | 0.12281 (0.00104) | 0.01468 (0.00038) |
| 0.05 | skew 0.3, not informative | −0.156957, 0.693147 | 0.23077 | 0.35817 | 0 | 0.99600 (0.00020) | 0.00400 (0.00020) |

Every share is far below its bound, and so is every crossed share. The scenarios at level 0.005 cover a re-size that stops every time (a reserve of 0), one that stops in part, and one that never stops. None of them appends none of its reserve: at this margin and level, 21 clusters never reach the count. The re-size stops every replicate of the ends null, and nearly every one of the skew 0.3 null: their spread asks for more clusters than the reserve holds. Almost none of their runs test the claim, so the table marks them not informative. Only skew 0.03 runs long enough to test the cap; it rejects 0.00300 and crosses 0.01468 against a level of 0.05. On this range no score reaches 1, so no factor reaches the cap's floor of 1/4; the fixed-n null tests of the claim test can reach it (0022).

The same design at a confirmatory study's planning values, an sd of 0.176 and an icc of 0.82, and at a third of that sd, with a reserve of 2,500 and the data at `power_at`:

| sd | icc | Power (se) | Stop share (se) |
|---|---|---|---|
| 0.176 | 0.82 | 0.08784 (0.00090) | 0.86528 (0.00108) |
| 0.0587 | 0.82 | 0.96963 (0.00054) | 0 |

Why:

- A pilot that sizes a confirmatory study is often too small to estimate the sd and icc well. Re-estimating from the study's own first batch corrects a wrong outside value; borrowing the outside value again carries its error into the count (Mütze et al.).
- Regulators prefer a blinded re-size, fixed in advance with a test that controls the error rate (EMA 2007). Adaptations on data that do not use the arm labels have little or no effect on the error rate, and where simulation is the evidence, about 100,000 replicates per null scenario, each on its own seed, suffice (FDA 2019). The rule must be binding and fully algorithmic, never an option a person may take or skip after seeing data (Żebrowska et al.); here the box runs it, and no person can.
- The square of the root mean square about 0 estimates the variance plus the squared effect, so s errs high, which is the safe side for power. One-sample estimates of this kind keep the error rate of a t-test (Kieser and Friede 2003; Maeda et al. 2026), and fall short of power only with very small first batches.
- The e-value test bets on each observation with a fraction fixed by the observations before it, and its bets do not depend on the planned count (Waudby-Smith and Ramdas). It runs over one sequence: the first batch, then the reserve in its fixed order. Under the null its wealth on that sequence is a nonnegative supermartingale that starts at 1, so by Ville's inequality the chance that it ever reaches 1 / level is at most the level. The e-value at the count the re-size picks is one point of that sequence, so it rejects with at most that chance, however the count was chosen. The null simulation checks this on the registration gate's model of the design, which shares the e-value, the clipped observation, the root mean square and the count search with the runner. Tests of whole runs pin what the model does not share: which reserve clusters the runner admits, their order, and the analysis.
- A design may depend on past data when each e-value stays valid given that past (Fischer and Ramdas). The usual way is fresh data; here the first batch both chooses the count and enters the e-value, and the validity comes from the bound over the whole sequence instead. The appended clusters are fresh.
- A pinned reserve fixes in advance which data the re-size adds, and their order. A released cluster was never measured, so a later study may use it.

Case against:

- Every re-size source studies t-tests. A blinded re-size of a non-inferiority t-test acts like an unblinded re-size of a superiority test, since the total variance carries the observed difference times the margin; its error excess is larger and persists at larger n (Glimm and Läuter; Friede and Kieser 2003, as cited by FDA 2019). A claim below a margin is a non-inferiority claim, and the root mean square here also carries the squared effect. The e-value test does not depend on the count being free of the effect, and the null simulation above is the evidence; a t-test would not inherit it.
- The count shows a function of s, which grows with the squared effect. A person who reads `needed` learns a little about the effect, and aggregate blinded statistics can unblind (Żebrowska et al.). The level holds whatever the count shows, but the count is not fully blind.
- The e-value test pays for its validity in power. At an sd of 0.176 and an icc of 0.82, 2,500 reserve clusters reach the count in only 13% of replicates, and the design passes 9% of the time; such a study fails the registration gate. At a third of that sd it never stops and passes 97% of the time. The rejection shares under the null, at most 0.00008 against a level of 0.005 and 0.00300 against 0.05, show how far the test sits inside its level.
- The rounded-up table makes the gate's count larger than the runner's, so the gate errs low on power. A replicate with an s past the table's top value stops in the gate even if the reserve could reach it.
- The model is normal within a stratum, and the reserve is assumed to come from the same population as the first batch. A reserve gathered later or from another source changes what the claim is about, though not its level.

Sources:

- FDA (2019), "Adaptive Designs for Clinical Trials of Drugs and Biologics", https://www.fda.gov/media/78495/download
- Kieser and Friede (2003), "Simple procedures for blinded sample size adjustment that do not affect the type I error rate", Statistics in Medicine 22(23), https://doi.org/10.1002/sim.1585
- Glimm and Läuter, "Some notes on blinded sample size re-estimation", https://ar5iv.labs.arxiv.org/html/1301.4167
- Maeda, Hattori and Friede (2026), "Blinded sample size re-estimation accounting for uncertainty in mid-trial estimation", https://arxiv.org/html/2602.03218v1
- Mütze, Schmidli and Friede, "Sample size re-estimation incorporating prior information on a nuisance parameter", https://ar5iv.labs.arxiv.org/html/1703.06957
- Żebrowska, Posch and Magirr, "Maximum type I error rate inflation from sample size reassessment when investigators are blind to treatment labels", Statistics in Medicine (2016), https://www.ebi.ac.uk/europepmc/webservices/rest/PMC4851240/fullTextXML
- EMA (2007), "Reflection paper on methodological issues in confirmatory clinical trials planned with an adaptive design", https://www.ema.europa.eu/en/documents/scientific-guideline/reflection-paper-methodological-issues-confirmatory-clinical-trials-planned-adaptive-design_en.pdf
- Waudby-Smith and Ramdas, "Estimating means of bounded random variables by betting", https://arxiv.org/abs/2010.09686
- Fischer and Ramdas, "Admissible online closed testing must employ e-values", https://arxiv.org/html/2407.15733v3
