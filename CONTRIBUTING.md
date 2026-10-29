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
2. Install Rust (stable, and 1.89 for the minimum version) and `pre-commit`.
3. Write a failing test first, then the code that makes it pass.
4. Run the checks before you push:
   ```
   cargo test --locked
   pre-commit run --all-files
   ```
5. Fill in the pull request template. Pull requests are not the place to
   design a feature; that happens on the issue.

## Rules for code

- A new dependency needs a line in `docs/decisions/0001-dependencies.md` with
  its reason. Open an issue before you add one.
- A new check, threshold or algorithm needs a decision record in
  `docs/decisions/`, with the numbers that justify it.
- New dependencies must have a license compatible with Apache-2.0.

## License of contributions

By contributing, you agree that your contribution is licensed under the
Apache-2.0 license of this project.

## Conduct

Everyone in this project follows the [code of conduct](CODE_OF_CONDUCT.md).
