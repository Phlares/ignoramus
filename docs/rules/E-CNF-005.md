# E-CNF-005

Severity: error

## Problem

Selection or confirmation data was used by an earlier study.

## Fix

Use data no earlier plan used for selection or confirmation, or change the role of this input.

## Why

Reused data has already shaped a decision. A test on it is not independent.

## Sources

- <https://doi.org/10.1038/s41562-016-0021>

## Applies to

- `inputs[*].path` (named:data-fresh), when mode = confirmatory
