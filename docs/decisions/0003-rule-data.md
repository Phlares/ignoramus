# 0003 Rules as data

The checker holds its rules as typed data in `rules/v0.toml`, not as scattered code.

Decision:

- One `[[rule]]` entry per diagnostic code. It holds the severity, the problem, the instruction for the agent, the reason, the public sources and the targets.
- A target is a field path and a check. Paths use dots, `[*]` for every array element, and `**` for any depth.
- Generic checks run from the data: `required`, `non-empty`, `type`, `enum`, `range`, `format`, `finite` and `known-key`.
- A check that compares several fields is a Rust function. The data refers to it as `named:<name>`. A name that no function carries is a load error.
- A code the CLI raises itself, not through a field path, has `check = "documented"` and no target. The data still records its text.
- `docs/rules/<code>.md` is generated from the data. A test fails when a page is stale.

Fail closed:

- If the plan log cannot be read, the data-reuse check reports a problem. It does not pass the plan.
- If `design.power` is missing, the power check reports a problem. It does not assume a default.
- If the stream a confirmatory plan takes its level from cannot be read, the check reports a problem (E-STR-001). It does not assume a level.

Why:

- One entry holds the text a person reads and the field it guards. They cannot drift apart.
- A new generic rule is a data edit with no new code.
- Docs from the same data are never out of date.

Sources are public: each entry cites a standard or paper by URL.
