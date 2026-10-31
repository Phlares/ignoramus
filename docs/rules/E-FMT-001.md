# E-FMT-001

Severity: error

## Problem

An id, a stratum or a study reference is not ASCII letters and digits joined by single hyphens.

## Fix

Use ASCII lowercase letters and digits, joined by single hyphens, for example my-study. A hypothesis id may also use capital letters, for example E1 or P1a. A stratum may be empty where the design or an input names none, but not in the strata a hypothesis lists. estimates.from and a param's from name a study id.

## Why

Ids and strata become file names, seed inputs and record keys, and are printed in diagnostics. Accents and look-alike letters would make two ids that look the same differ.

## Sources

- <https://toml.io/en/v1.0.0>

## Applies to

- `study.id` (format)
- `question.hypotheses[*].id` (format)
- `design.conditions[*].id` (format)
- `outcomes[*].id` (format)
- `design.stratum` (format)
- `inputs[*].stratum` (format)
- `question.hypotheses[*].strata[*]` (format)
- `question.hypotheses[*].estimates.from` (format)
- `design.conditions[*].params.*.from` (format)
