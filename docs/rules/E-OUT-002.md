# E-OUT-002

Severity: error

## Problem

An outcome does not say which direction is better.

## Fix

Set direction to lower or higher, whichever is better for this outcome.

## Why

The test needs to know which way counts as an improvement. Choosing after the data are in would bias the result.

## Sources

- <https://doi.org/10.1136/bmj.e7586>

## Applies to

- `outcomes[*].direction` (required)
- `outcomes[*].direction` (enum)
