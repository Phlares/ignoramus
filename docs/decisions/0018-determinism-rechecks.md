# 0018 Determinism rechecks

A plan can declare that its payload is deterministic: the same job always gives the same output. The box tests the claim on a sample of its own rows, and a study that borrows estimates can carry a passed test from a study that ran the same code.

Decision:

- **The field.** `pins.deterministic`, a boolean, `false` by default.
- **One copy per condition.** With the field set, the schedule holds one recheck copy for each condition id. For the j-th condition id, in order of first appearance, the box draws one of that condition's rows from the seed `recheck/<condition id>` (0009). The copy takes index `rows + j` and `recheck_of`, the original's index. It keeps every other field of the original, and runs right after it. A schedule without copies keeps its bytes: `recheck_of` is left out of it. Copies count in the estimate, in `status` and in the budget, as any row does.
- **The same job.** A copy gets its original's job: the same arguments, params, `IGNORARIUM_SEED`, and the original's index as `IGNORARIUM_ROW`. Only its output folder is its own. A resumed run runs a copy with the original's job too, after a pause between the two rows.
- **The comparison.** After a copy's last attempt, the box reads its original's signed row record. The copy's record holds `recheck_of` and `same`:
  - `true` when the exit and the output hash are both equal, `false` when either differs; an exit that alone differs is a mismatch.
  - `null` when either exit is `void:*`: a void row's exit and output need not be the payload's.
  - `null` when the two records name another environment or another isolation level. The box then writes a `row.recheck` warning event that names the keys that differ. A pause between the rows can change either, and a difference between two environments or levels is no behaviour of the payload either.

  An original without a row record, or one without an exit or an output hash, is a records fault.
- **Stop or flag, by mode.** In a confirmatory study, `same: false` stops the run: the box writes a stop record, `row <i> gave output <a> and its recheck row <j> gave <b>; the payload is not deterministic`, as `stop` does (0006). The study writes no `analysis` record. A run that starts on a confirmatory study whose records hold a copy with `same: false` stops at once, before any row, so a crash between the copy's record and the stop record cannot reach the verdicts. In an exploratory study a mismatch writes a `row.recheck` warning event, and the run goes on. A match writes the same event at level `info`.
- **The result.** Copies add no paired values and no selection units. The `analysis` record holds `recheck`: `null` without the field, else `{rows, mismatched, uncompared, carried_from}`. `rows` counts the compared copies, `mismatched` and `uncompared` list copy indices in the order they ran. The check passed when no compared copy differed, and either every copy was compared (at least one) or the study carried a check.
- **The carry.** A plan that declares the field runs no copies when it carries the check of a study it takes estimates from (0017). It carries the first such study that is finished, whose check passed, and whose payload and environment the plan pins. Other model or calibration inputs do not bar a carry: they are data the code reads, not the code. The isolation level is not part of the match, since levels 1 and 2 run rows in the same pinned image and hash outputs with one function (0015). The plan's `scope.min_isolation` must be 1 or more. Level 0 runs rows on the host root and ignores `pins.environment`, so a carry there would claim an environment no row ran in. `check` decides the carry and `register` freezes it, so the schedule, which is written once, never changes with a later run:
  - `check` prints `determinism: carried from <study>` or `determinism: <n> recheck rows`. Its JSON always holds a `recheck` key: `{"carried_from": <study or null>, "rows": <n>}` for a plan that declares the field and has an estimate, `null` otherwise.
  - `register` records `carries: {study, record}`, with the hash of that study's `analysis` record, and prints `carries the determinism check of <study>`. A run refuses when that record has gone from the box.
  - The finished study's `recheck` names `carried_from`, and counts as passed for the next borrower.

Why:

- A build is reproducible when the same source, environment and instructions give bit-for-bit identical artifacts, checked by comparing hashes (reproducible-builds.org). A row is such a build of its output from its job.
- Malka, Zacchiroli and Zimmermann rebuilt 709,816 historical Nix packages: 69 to 91% came out bit for bit identical, and an embedded build date was the largest known cause of a difference, about 15%. A payload that writes the time fails the same way.
- Adams and co-authors let a result that must come back identical be computed again only by a deterministic process, with every seed kept, and with a stored hash of the result to check it. SimFS keeps a checksum of each output file of the first run for the same reason. The copy gets its original's seed, and its output hash is compared with the original's.
- Both papers weigh storing a result against computing it again. A signed check that passed for the same code is a stored result, so a borrower takes it instead of paying for its own copies.
- A confirmatory verdict rests on the payload's output for each job. A payload that gives another output for the same job could give another verdict, so the study stops, and a stop cannot be undone. An exploratory study only flags it.

Limits:

- The check runs on one machine and at one level: the copy runs right after its original, on the same box. Comparisons across machines, levels or payload versions stay with the study's own tools.
- One row per condition is a sample. A payload that differs only on some units can pass.
- A carry covers the code, not the inputs. A payload deterministic on one input may not be on another.
- The output hash covers every file in the output folder, so a log line with a time or a host fact fails the check.
- E-UNT-002 counts the rows without the copies, so a schedule can hold up to one row per condition more than its 100,000-row cap.

Cost: the copies cost one row per condition, unless the plan carries a check.

Sources:

- https://reproducible-builds.org/docs/definition/
- Malka, Zacchiroli and Zimmermann (2025), "Does Functional Package Management Enable Reproducible Builds at Scale? Yes.", https://arxiv.org/html/2501.15919v2
- Di Girolamo and co-authors (2019), "SimFS: A Simulation Data Virtualizing File System Interface", https://ar5iv.arxiv.org/html/1902.03154
- Adams, Long, Miller, Pasupathy and Storer (2009), "Maximizing Efficiency By Trading Storage for Computation", HotCloud 2009, https://static.usenix.org/events/hotcloud09/tech/full_papers/adams/adams_html/
