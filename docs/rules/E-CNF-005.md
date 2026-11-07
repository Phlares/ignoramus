# E-CNF-005

Severity: error

## Problem

Confirmation or reserve data was used by an earlier study.

## Fix

Use data no earlier plan used for confirmation or reserve, or change the role of this input. Selection data may be reused.

## Why

Reused data has already shaped a decision. A test on it is not independent. A reserve input is confirmation data held back, so it must be fresh too.

## Sources

- <https://doi.org/10.1038/s41562-016-0021>

## Applies to

- `inputs[*].path` (named:data-fresh), when mode = confirmatory
