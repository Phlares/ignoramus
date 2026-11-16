# 0014 References between studies

Date: 2026-10-31.

Amended: 2026-11-15. The matching rule for `selected` reads as 0021 set it (#38); the text held the earlier rule until now (#118).

A follow-on study is written and checked before the study it builds on has run. Some of its values exist only in that earlier study's records: the sd and icc that size it, and the condition a selection stage picked. The author never types such a value or a record hash. The spec names the study, and the box finds the records.

Decision:

- **One form.** A reference is `{ from = "<study id>" }`. Its meaning comes from where it sits. In a hypothesis's `estimates`, it gives the sd and icc of that hypothesis's contrast. In a condition's `params`, it gives the condition id that study's selection stage picked. A reference must name another study by its id. A record hash, or any text that is not a study id, is refused.
- **Estimates.** At the end of a run the box signs estimate records, per hypothesis and per stratum in scope plus the pooled group: clusters, units, the mean of the contrast's unit values, the unit variance (between plus within clusters, by one-way analysis of variance), and the icc (none with fewer than 2 clusters). Each estimate carries a key: the outcome, measure, paired rule and repeats, the reference condition with its params, and the contrast's conditions, weights and resolved params.
- **Finished.** A study is finished when its first `analysis` record is followed by every estimate and verdict that record promises, and its stream slot is closed. Only the records the analysis record promised count as the study's results.
- **Matching.** A hypothesis takes the estimate with its own contrast's key, in the same stratum, else in the pooled group. An estimate with no icc, or no variance above 0, is skipped in favour of the pooled one, and `register` notes that it used the pooled estimate. Several matching estimates must agree. For a contrast on `selected`, the box looks up each candidate. A mean claim takes one candidate's own sd and icc, the one 0021 names. A quantile claim takes the largest sd and the largest icc. Until 0021 every claim took the largest of each.
- **Pending.** A reference to a study that is not registered here, or has not finished, is pending: `check` warns (`W-REF-001`), skips the gates that need the value, and exits 3 (no errors, values pending). A study that has not finished is pending even when another pending reference leaves the contrast's key unknown. A finished study without a match is refused: no matching estimate, or no selection record.
- **Freezing.** `register` refuses a plan with a pending reference. It resolves the rest and writes each value into the register record under `resolved`: the path in the plan, the study, the hashes of the records the value comes from, and the value. `run` reads condition params from that record and passes them to the payload as canonical JSON in `IGNORARIUM_PARAMS`. `run` refuses to start when a frozen record is gone from the study's log.

Why: a value the author can type is a value the author can invent. The box reads it from signed records, so every number that sizes a study or picks its condition traces to a record hash.

Cost: the earlier study must run in the same box. An estimate from another box needs an import path.

Case against:

- The reason says every number that sizes a study traces to a record hash. The margin, `power_at`, the range and the power target stay typed (0030). In the first confirmatory study the size ran from about 270 to about 420 clusters over the interval of the borrowed spread; a nearer typed ceiling alone took one stratum from 276 clusters to 135 (0030).
- For a quantile claim on `selected`, the matching rule still pairs the largest sd of one candidate with the largest icc of another. 0021 said that pair "describes none" and dropped it for mean claims, five days after this record.
- Two other forms were proposed and not taken. Issue #9 keeps a typed value and checks it against a named record. Issue #8 names the parent spec and the field. Either keeps the number or its exact source readable in the spec. Here the box finds the estimate by a key, a pooled fallback and a rule over candidates.
- The rule ranks where a number comes from above how good it is. A study with 5 clusters per stratum in the same box may lend; in the first confirmatory study one borrowed icc had a 95% interval from 0 to 0.66 (0030). A larger study from another box may not lend.

(written 2026-11-15)

What failure this prevents: a study sized from an sd or an icc that its author typed, or run on a condition its author picked by hand, with no signed record behind the value. Expected, not observed. (written 2026-11-15)

Constants: none. The record fixes a form and matching rules, not a value. (written 2026-11-15)

Sources: none. The record cites no outside work. (written 2026-11-15)
