# 0028 Box identity

Date: 2026-11-15.

Amended: 2026-11-15. The binary gate and `--box-changed` (#104).

Amended: 2026-11-21. A stamped study has an outside anchor for the register record (0033, #122); the text said the box had none until now (#129).

A box has two identities: the rules it embeds and the binary that runs. Each has a SHA-256 hash. Nothing in a study's verdict reads either hash. Only `pins.rule_base` and the binary gate (see Gate) can refuse a plan or a run.

Decision:

- **Rules hash.** It covers the rule data in `rules/v0.toml` and `rules/scan.toml`, as embedded in the binary. Each file enters the hash with its byte length as a big-endian `u64` prefix, so no text moves across the boundary between the two files. It prints as `sha256:<hex>`. `ignorarium check` prints it as `rules:`.
- **What the rules hash does not cover.** It does not cover the logic of the named rules in `named.rs`. A change to that code leaves the rules hash as it was. The binary hash covers behaviour.
- **Binary hash.** It is the SHA-256 of the running executable, read from `/proc/self/exe`. A binary that cannot read itself records `{"unavailable": "<why>"}`. It is never an error.
- **`pins.rule_base`.** An empty string pins nothing and passes. A set value must equal the rules hash of this box, or rule E-HSH-002 refuses the plan at `check`. The rule reads no other pin.
- **Accident, not tampering.** The hashes catch accidents: a wrong binary, or a rebuilt one. They do not catch tampering. A party who can change the binary can change the hash it reports. Catching that needs an outside anchor for the hash and an independent rebuild. A stamped study has the first for the registered hashes: the stamp covers the hash of the register record, which holds both (0033). The box has no independent rebuild yet.
- **No reproducibility level.** The box claims none. It does not state that the same source gives the same binary, as the definition at reproducible-builds.org requires.

Record. `register` writes both hashes into the register record, under `box`. `verify --study` prints them, or `not recorded` for a record written before the field existed. An older box reads a register record that holds `box` as no register (`deny_unknown_fields`), as for `gate`.

Evidence: in the acceptance run, the box binary's hash was written into the decision log by hand. It was compared by hand during the run. No code did either step.

Why:

- No record stated which rules or which binary checked and ran a study. A box rebuilt with changed rules left the same records as the first one (#67).
- The records held the crate version, which does not change between commits (#67).
- `pins.rule_base` was a required spec field that nothing read (#67).
- The gate is on the binary, not on the rules hash (Gate). A refusal on a changed rules hash would only block a build that fixes rule text (#67, comment of 2026-11-15).

(written 2026-11-21)

Case against:

- An operator can always pass `--box-changed`. The gate slows a change; it does not lock it. The gain is a dated, signed declaration in place of a reconstruction.
- No record names the source commit of the binary, and the crate version in the records is `0.0.1` at every commit (#90). A reader cannot match a binary to its source, so each hash stays the binary's report on itself (Decision).
- The rules hash is a second identity that the binary hash already covers (Gate). It does not cover the code of the named rules (Decision), which 53 of the 75 rules call (0003, Case against). `pins.rule_base`, the one field a rule compares with it, is empty in both templates.

(written 2026-11-21; the first bullet is from Gate, where #104 wrote it)

Limits:

- `report` is not gated (Gate). It writes a signed `opened` record with an empty body (`cli/report.rs`). The study's log then does not name the binary that built a report or a card; only the card itself does.
- The gate can write a deviation where nothing deviated. A study that an older box finished gets its `done` record through one more run (0034). With a different binary and a recorded one to compare, that run needs `--box-changed` and adds a `box-changed` row. Its `impact` reads "rows from here run under ...", and no row runs.

(written 2026-11-21)

What failure this prevents: a study that continues under a wrong or rebuilt binary with no trace in its records; and a plan whose `pins.rule_base` no code compares with the rules. The first gap was observed in the acceptance run: the binary's hash was written and compared by hand (Evidence; #67). No wrong binary was observed. (written 2026-11-21)

Constants: none. This record fixes what each hash covers and when the gate refuses. SHA-256, the `u64` length prefix and the exit codes are parts of a format, not tuned values. (written 2026-11-21)

Sources:

- Reproducible Builds, "Definition": https://reproducible-builds.org/docs/definition/

## Gate

- **On the binary, not the rules.** The run path reads no rule data. `check_all` runs only from `check` and `family::check`. The runner calls `rules::selection_data`, `family::exempt` and `family::shared`, which are code. The rules act at `check` and `register`, where their hash is recorded and pinned. The binary holds the rules, so one gate on the binary hash covers both.
- **Baseline.** Once `run` or `resume` holds the run lock, it compares the box binary with the binary of the last `session.start` record in the log. With no session, it compares with the register record's `box`. A binary that cannot read itself never counts as the same.
- **Refusal.** A different binary refuses with exit 1: `the box binary changed since the last session (<old> to <new>); to continue, run with --box-changed "<reason>"`. The box writes no session record and runs no row. A refused `resume` leaves a pending pause in place: the box sends the resume request only after the gate passes.
- **Declared path.** `run --box-changed <reason>` and `resume --box-changed <reason>` pass the gate. The box then writes, in this order:
  1. a row in `deviations.jsonl`, kind `box-changed`, with the reason and the timing; `original` names the old binary and rules hashes, `justification` is the reason, and `impact` names the new binary and rules hashes;
  2. the signed `session.start` record, which also holds `changed_from`, the old binary hash, and `reason`.
  The card's deviation table then shows the change with its timing stage, with no hand entry. The date is in the signed session record. An empty reason is a usage error (exit 2).
- **A declaration of nothing refuses.** `--box-changed` on the binary the last session ran refuses with exit 1: `--box-changed was given, but the box binary is the one the last session ran`.
- **A live run.** A `resume` that only sends its request to a live run ignores the flag. The live run keeps its binary.
- **Not gated.** `report`, `verify`, `status` and every other verb. A card rebuild with `report --card` stays free.
- **Old studies.** A register record without `box`, and no session, gives the gate nothing to compare. The first run under this box passes, ignores `--box-changed`, and writes `session.start` with its binary. That session is the baseline from then on. `verify` prints `not recorded` for the register hashes.
- **`verify`.** `verify --study` prints one line per binary change between consecutive session starts, or between the register record and the first start, where `k` counts session starts from 1: `binary changed at session <k>: declared (<reason>)`, or `binary changed at session <k>: undeclared`. A change is declared only when the start's `changed_from` is the binary before it. An undeclared change cannot occur through the box. `verify` also lists it as a failure, `undeclared binary change at session <k>`, and exits 1.
