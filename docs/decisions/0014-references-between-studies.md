# 0014 References between studies

A follow-on study is written and checked before the study it builds on has run. Some of its values exist only in that earlier study's records: the sd and icc that size it, and the condition a selection stage picked. The author never types such a value or a record hash. The spec names the study, and the box finds the records.

Decision:

- **One form.** A reference is `{ from = "<study id>" }`. Its meaning comes from where it sits. In a hypothesis's `estimates`, it gives the sd and icc of that hypothesis's contrast. In a condition's `params`, it gives the condition id that study's selection stage picked. A reference must name another study by its id. A record hash, or any text that is not a study id, is refused.
- **Estimates.** At the end of a run the box signs estimate records, per hypothesis and per stratum in scope plus the pooled group: clusters, units, the mean of the contrast's unit values, the unit variance (between plus within clusters, by one-way analysis of variance), and the icc (none with fewer than 2 clusters). Each estimate carries a key: the outcome, measure, paired rule and repeats, the reference condition with its params, and the contrast's conditions, weights and resolved params.
- **Finished.** A study is finished when its first `analysis` record is followed by every estimate and verdict that record promises, and its stream slot is closed. Only the records the analysis record promised count as the study's results.
- **Matching.** A hypothesis takes the estimate with its own contrast's key, in the same stratum, else in the pooled group. An estimate with no icc, or no variance above 0, is skipped in favour of the pooled one, and `register` notes that it used the pooled estimate. Several matching estimates must agree. For a contrast on `selected`, the box looks up each candidate and takes the largest sd and the largest icc.
- **Pending.** A reference to a study that is not registered here, or has not finished, is pending: `check` warns (`W-REF-001`), skips the gates that need the value, and exits 3 (no errors, values pending). A study that has not finished is pending even when another pending reference leaves the contrast's key unknown. A finished study without a match is refused: no matching estimate, or no selection record.
- **Freezing.** `register` refuses a plan with a pending reference. It resolves the rest and writes each value into the register record under `resolved`: the path in the plan, the study, the hashes of the records the value comes from, and the value. `run` reads condition params from that record and passes them to the payload as canonical JSON in `IGNORARIUM_PARAMS`. `run` refuses to start when a frozen record is gone from the study's log.

Why: a value the author can type is a value the author can invent. The box reads it from signed records, so every number that sizes a study or picks its condition traces to a record hash.

Cost: the earlier study must run in the same box. An estimate from another box needs an import path.
