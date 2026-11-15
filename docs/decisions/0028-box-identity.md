# 0028 Box identity

A box has two identities: the rules it embeds and the binary that runs. Each has a SHA-256 hash. Nothing in a study's verdict reads either hash. Only `pins.rule_base` and the binary gate (see Gate) can refuse a plan or a run.

Decision:

- **Rules hash.** It covers the rule data in `rules/v0.toml` and `rules/scan.toml`, as embedded in the binary. Each file enters the hash with its byte length as a big-endian `u64` prefix, so no text moves across the boundary between the two files. It prints as `sha256:<hex>`. `ignorarium check` prints it as `rules:`.
- **What the rules hash does not cover.** It does not cover the logic of the named rules in `named.rs`. A change to that code leaves the rules hash as it was. The binary hash covers behaviour.
- **Binary hash.** It is the SHA-256 of the running executable, read from `/proc/self/exe`. A binary that cannot read itself records `{"unavailable": "<why>"}`. It is never an error.
- **`pins.rule_base`.** An empty string pins nothing and passes. A set value must equal the rules hash of this box, or rule E-HSH-002 refuses the plan at `check`. The rule reads no other pin.
- **Accident, not tampering.** The hashes catch accidents: a wrong binary, or a rebuilt one. They do not catch tampering. A party who can change the binary can change the hash it reports. Catching that needs an outside anchor for the hash and an independent rebuild. The box has neither yet.
- **No reproducibility level.** The box claims none. It does not state that the same source gives the same binary, as the definition at reproducible-builds.org requires.

Record. `register` writes both hashes into the register record, under `box`. `verify --study` prints them, or `not recorded` for a record written before the field existed. An older box reads a register record that holds `box` as no register (`deny_unknown_fields`), as for `gate`.

Evidence: in the acceptance run, the box binary's hash was written into the decision log by hand. It was compared by hand during the run. No code did either step.

## Gate

- **On the binary, not the rules.** The run path reads no rule data. `check_all` runs only from `check` and `family::check`. The runner calls `rules::selection_data`, `family::exempt` and `family::shared`, which are code. The rules act at `check` and `register`, where their hash is recorded and pinned. The binary holds the rules, so one gate on the binary hash covers both.
- **Baseline.** Once `run` or `resume` holds the run lock, it compares the box binary with the binary of the last `session.start` record in the log. With no session, it compares with the register record's `box`. A binary that cannot read itself never counts as the same.
- **Refusal.** A different binary refuses with exit 1: `the box binary changed since the last session (<old> to <new>); to continue, run with --box-changed "<reason>"`. The box writes no session record and runs no row.
- **Declared path.** `run --box-changed <reason>` and `resume --box-changed <reason>` pass the gate. The box then writes, in this order:
  1. a row in `deviations.jsonl`, kind `box-changed`, with the reason and the timing; `original` names the old binary and rules hashes, `justification` is the reason, and `impact` names the new binary and rules hashes;
  2. the signed `session.start` record, which also holds `changed_from`, the old binary hash, and `reason`.
  The card's deviation table then shows the change, dated, with no hand entry.
- **A declaration of nothing refuses.** `--box-changed` on the binary the last session ran refuses with exit 1: `--box-changed was given, but the box binary is the one the last session ran`.
- **A live run.** A `resume` that only sends its request to a live run ignores the flag. The live run keeps its binary.
- **Not gated.** `report`, `verify`, `status` and every other verb. A card rebuild with `report --card` stays free.
- **Old studies.** A register record without `box`, and no session, gives the gate nothing to compare. The first run under this box passes, ignores `--box-changed`, and writes `session.start` with its binary. That session is the baseline from then on. `verify` prints `not recorded` for the register hashes.
- **`verify`.** `verify --study` prints one line per binary change between consecutive session starts, or between the register record and the first start: `binary changed at session <k>: declared (<reason>)`, where `k` counts session starts from 1. A change whose start has no `changed_from` cannot occur through the box. `verify` reports it as `undeclared binary change at session <k>` and exits 1.
- **Case against.** An operator can always pass the flag. The gate slows a change; it does not lock it. The gain is a dated, signed declaration in place of a reconstruction.

## Sources

- Reproducible Builds, "Definition": https://reproducible-builds.org/docs/definition/
