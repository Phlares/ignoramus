# 0021 The power gate sizes from what the test sees

The registration power gate (0013) borrows an sd and an icc from the study a hypothesis names in `estimates` (0014). It now borrows the spread the test will see: one arm's own pair, and for a mean claim, the source study's unit values clipped to the claim's declared range.

Decision:

- **One arm's pair.** A hypothesis can stand for several contrasts: with `selected`, one per candidate of the selection stage. Per group (each stratum the claim lists, else the pooled group), the gate takes the sd and icc of one contrast: the one whose cluster mean has the largest sd at `design.units_per_cluster` units, sd × √(icc + (1 − icc) / m). A tie keeps the first. It never pairs the largest sd of one arm with the largest icc of another.
- **Clipped to the declared range.** For a claim whose statistic is `mean = [lo, hi]`, the box reads the source study again: the plan text it stored at registration, which must hash to the value its `register` record holds, and its signed row records. It rebuilds the unit values its estimate was fitted from, with the same pairing, contrast, clusters and analysis of variance as the source's own analysis. The source's `inputs` record and the data hashes its rows name tell which inputs hold the same data and so count as one cluster; its `resize` record tells which reserve clusters ran.
- **When the record stands.** If every unit value of the group lies in [lo, hi], clipping changes nothing, and the gate takes the source's estimate record as it is. Otherwise it clips each unit value to [lo, hi] and fits again. The range is the borrowing plan's, not the source's: it is the range the new test clips to.
- **Fail closed.** A stored plan that is missing, does not parse or is not the registered one, a missing `register` or `inputs` record, rows that name other data than the `inputs` record or than each other, rows that do not give the clusters and units the estimate counts, a `resize` record that does not parse, or a plan without the hypothesis the estimate names: each makes the estimate missing (E-CNF-003), with the reason. The gate never falls back to the raw values. A clipped fit with no icc or no variance above 0 is missing too.
- **What stays.** The gate's model, its simulation and the text of a shortfall (E-CNF-002) do not change. A quantile claim reads the estimate record as before.

Example. A source study measures 4 texts, 2 units each. Three texts give unit values from −2 to 4. The fourth, a text unlike the others, gives 200 and 180. The next plan claims a mean in [−50, 50], so its test reads each unit of the fourth text as 50. The raw values fit a unit sd of 94.5 with an icc of 0.994; the clipped values fit an sd of 24.4 with an icc of 0.992. Sized from the raw values, the gate would draw cluster means about 4 times as spread as any the test can see.

Why:

- A test of a mean has power only under a bound declared before the data (Bahadur and Savage 1956, as cited by Wang and Ramdas, and by Mineiro). The declared range is that bound here, and the claim is about the clipped mean (0013). A spread measured past the range belongs to a statistic the test never computes.
- Clipping at a bound, then averaging, is a known way to estimate a mean under heavy tails (Lugosi and Mendelson). A text far outside the range is the heavy-tail case, and it dominates a raw sd.
- One arm's own sd and icc describe a distribution that exists. The largest sd of one arm with the largest icc of another describes none, and sizes for a worse case than any arm. Sizing from plausible spreads and checking the worst is the practice for a borrowed sd (Bell et al.); the worst arm, taken whole, is that check over the arms.
- A range that clips nothing gives the same values, so the stored estimate is the one source of truth, and a test pins that the refit equals it bit for bit.

Case against:

- The clipped spread is the source's. A borrowing study whose texts lie further outside the range than the source's sees less spread than the gate assumes in some strata and more in others; the gate still assumes one normal model per stratum (0013).
- A range that clips much of the source makes the claim about a quantity far from the raw mean. The gate does not warn about it; the range is the plan's choice.
- A source whose records lack the data hashes or the `inputs` record, or whose stored plan is gone, cannot lend to a mean claim at all. That is the cost of failing closed.
- The arms are ranked at `design.units_per_cluster`. When inputs give fewer units than that, the gate's own cluster size is smaller, and another arm can have the wider cluster mean there.

Sources:

- Wang and Ramdas, "Catoni-style confidence sequences for heavy-tailed mean estimation", https://arxiv.org/abs/2202.01250
- Mineiro, "A lower confidence sequence for the changing mean of non-negative right heavy-tailed observations with bounded mean", https://arxiv.org/abs/2210.11133
- Lugosi and Mendelson, "Mean estimation and regression under heavy-tailed distributions: a survey", https://arxiv.org/abs/1906.04280
- Bell, Whitehead and Julious (2018), "Guidance for using pilot studies to inform the design of intervention trials with continuous outcomes", Clinical Epidemiology 10, https://www.ebi.ac.uk/europepmc/webservices/rest/PMC5779280/fullTextXML
