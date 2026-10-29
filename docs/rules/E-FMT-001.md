# E-FMT-001

Severity: error

## Problem

An id is not ASCII kebab-case.

## Fix

Use ASCII lowercase letters and digits, joined by single hyphens, for example my-study.

## Why

Ids become file names and seed inputs. Accents and look-alike letters would make two ids that look the same differ.

## Sources

- <https://toml.io/en/v1.0.0>

## Applies to

- `study.id` (format)
- `design.conditions[*].id` (format)
- `outcomes[*].id` (format)
