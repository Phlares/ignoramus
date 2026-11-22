# 0003 Rules as data

Date: 2026-10-29.

Amended: 2026-10-31. The power check asks only for `design.power`; an unreadable stream is a problem, not a default level (#31).

Amended: 2026-11-21. The list of generic checks names `unique`, which the code has held since #1 (#129).

The checker holds its rules as typed data in `rules/v0.toml`, not as scattered code.

Decision:

- One `[[rule]]` entry per diagnostic code. It holds the severity, the problem, the instruction for the agent, the reason, the public sources and the targets.
- A target is a field path and a check. Paths use dots, `[*]` for every array element, and `**` for any depth.
- Generic checks run from the data: `required`, `non-empty`, `type`, `enum`, `range`, `format`, `finite`, `known-key` and `unique`.
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

Case against:

- Most rules are code. Of the 75 rules, 12 use only generic checks; 49 use only named functions, 4 use both, and 10 are `documented`. Only the 12 are "a data edit with no new code" (Why). #66 says the named rules "are code, not data".
- The data ships inside the binary, so a data edit still needs a build. The binary embeds `rules/v0.toml` (`rules/mod.rs`), and an edit of that file changes nothing in a built binary (#67). A fault in the data shows only when the binary first reads its rules or looks a code up. As Rust code it would fail the build.
- The spec's shape is stated twice: in the typed model (`spec/model.rs`) and in the 152 `type` and 93 `required` targets of `rules/v0.toml`. The two can disagree: `rules/mod.rs` raises `E-TYP-001` for a spec that passes every rule and still does not fit the model. The typed model alone would state the shape once.
- A standard schema language could carry the generic checks to a writer that cannot run the box. #68 found that a writer with no shell cannot loop on `check`, and that "A JSON Schema or a hosted `check` would let it".

(written 2026-11-21)

What failure this prevents: a diagnostic whose text and guarded field drift apart (Why); a rule page that describes a check the binary no longer runs (Why); and a plan that passes because a value its check needs is missing or unreadable (Fail closed). Expected, not observed in a study. (written 2026-11-21)

Constants: none. This record fixes where the rules live and how they are read. Each rule's own values sit in its entry in `rules/v0.toml`. (written 2026-11-21)

Sources are public: an entry cites each standard or paper by URL. An entry may cite none.

Sources: none. The record cites no outside work. (written 2026-11-21)
