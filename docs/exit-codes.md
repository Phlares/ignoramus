# Exit codes

`ignorarium dev gen-exit-codes` writes this page from `crates/ignorarium/src/cli/exit.rs`. Do not edit it by hand.

| Code | Name | Meaning |
|---|---|---|
| 0 | done | The verb did its work. For `check` and `review`: the plan has no error and no value is pending. For `verify`: every check passed. |
| 1 | refused | The box did the work and the answer is no. Examples: `check` found an error in the plan; the study is not registered, not finished, paused or stopped; `verify` found a fault. |
| 2 | fault | The box could not do the work. Examples: a command line it cannot parse; a file or key it cannot read or write. |
| 3 | pending | Only `check` and `review`: the plan has no error, and at least one value waits on a study that has not finished. |
| 102 | selftest-failed | Only `dev selftest`: a check of the sandbox failed. |

## Notes

- `--help` and `--version` exit 0.
- A study's record log that fails verification (a torn line, a bad hash, signature or chain, a newer format) ends `report` and `verify --study` with 1, and `status`, `run`, `resume`, `pause`, `stop` and `stamp --request` with 2.
