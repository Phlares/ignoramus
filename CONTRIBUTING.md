# Contributing to ignorarium

> [!NOTE]
> ignorarium is a personal project. I maintain it in my free time and I read every
> issue myself. Please read this page before you open one. Thank you.

## The one rule

**Understand what you submit.** If you can't explain what your change does and
how it fits the rest of ignorarium without help from an AI tool, don't send it. AI
tools are fine to use; see the [AI policy](AI_POLICY.md).

## How to contribute right now

Open an issue. That is the way in for now.

- **Bug:** use the bug form. It needs the steps, the expected result, the actual
  result and the ignorarium version (`ignorarium --version`).
- **Idea:** use the idea form. Say what you can't do today, not only the fix you
  have in mind.
- **Security problem:** don't open an issue. Use "Report a vulnerability" on the
  Security tab. See [SECURITY.md](SECURITY.md).

Pull requests are invite-only for now. When I accept an issue (label
`accepted`), I either fix it or invite you to. An accepted issue is ready to
work on: the problem is understood and the approach is agreed.

If an open issue already matches yours, add a thumbs-up reaction instead of a "+1"
comment. Every comment emails everyone on the issue.

## If I invite you to send a pull request

1. Work against the accepted issue. One issue per pull request; keep it small.
2. Install Rust (stable, and 1.89 for the minimum version), `pre-commit` and
   `uv`.
3. Write a failing test first, then the code that makes it pass.
4. Run the checks before you push. CI runs the same list, `gate.toml`:
   ```
   uvx --from git+https://github.com/Phlares/workflows@v1.11.0 gate
   ```
   The last line it prints is the result. Paste it into the pull request.
5. Fill in the pull request template. Pull requests are not the place to
   design a feature; that happens on the issue.

## Rules for code

- A new dependency needs a line in `docs/decisions/0001-dependencies.md` with
  its reason. Open an issue before you add one.
- A new check, threshold or algorithm needs a decision record in
  `docs/decisions/`, with the numbers that justify it. See
  [Decision records](#decision-records).
- New dependencies must have a license compatible with Apache-2.0.

## Decision records

A record under `docs/decisions/` is named `NNNN-<slug>.md` and holds the
parts below. Each label starts a line and ends with a colon. The test
`crates/ignorarium/tests/docs_decisions.rs` checks them.

```text
# NNNN Title

Date: YYYY-MM-DD. The day the record goes on file.

Decision:

- What is decided.

Why:

- The reasons.

Case against:

- The arguments against the decision, and the alternatives rejected.

What failure this prevents: the failure, and whether it was observed or is only expected.

Constants:

- `name` = value. Basis: where the value comes from. Rule for change: when and how it may change.

Sources:

- A citation with its URL.
```

- **Date.** The date is the day the record goes on file. When the decision
  was made earlier, say so on the same line.
- **Case against.** A limit or a cost alone is not a case against.
- **What failure this prevents.** Name the source of an observed failure.
- **Nothing to say.** Write `none` and the reason, as in
  `Constants: none. This record sets no value.` A bare `none` fails the test.
- **Later changes.** A change to the Decision or Constants part adds a line
  under the Date line, after a blank line:
  `Amended: YYYY-MM-DD. What changed (#PR).` Oldest first. A spelling fix
  needs none.
- **Parts added later.** A part written on a later day than the record's
  date ends with `(written YYYY-MM-DD)`.
- **Other parts** are free: Evidence, Limits, Cost, Open. The order is free.
  A line that starts with a capital letter and has a colon before any full
  stop reads as a label. It ends the part above it.
- **Records 0001 to 0029** are older than this template. Their Date lines,
  and their Amended lines dated up to that day, were added on 2026-11-15
  from the git history. 0001 has no Amended line: each dependency is a list
  entry, and git holds its date.

## License of contributions

By contributing, you agree that your contribution is licensed under the
Apache-2.0 license of this project.

## Conduct

Everyone in this project follows the [code of conduct](CODE_OF_CONDUCT.md).
