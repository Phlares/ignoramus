# 0028 Box identity

A box has two identities: the rules it embeds and the binary that runs. Each has a SHA-256 hash. Nothing in a study's verdict reads either hash. Only `pins.rule_base` and the binary gate (added with #81) can refuse a plan or a run.

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

Added with #81.

## Sources

- Reproducible Builds, "Definition": https://reproducible-builds.org/docs/definition/
