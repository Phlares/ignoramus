# E-FMT-001

Severity: error

## Problem

An id is not ASCII kebab-case.

## Fix

Use ASCII lowercase letters and digits, joined by single hyphens, for example my-study. A stratum may be empty when there are none.

## Why

Ids and strata become file names, seed inputs and record keys, and are printed in diagnostics. Accents and look-alike letters would make two ids that look the same differ.

## Sources

- <https://toml.io/en/v1.0.0>

## Applies to

- `study.id` (format)
- `design.conditions[*].id` (format)
- `outcomes[*].id` (format)
- `design.stratum` (format)
- `inputs[*].stratum` (format)
