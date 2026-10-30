# E-CNF-003

Severity: error

## Problem

The variance record is missing, is not an estimate for an outcome of this plan, or holds another value.

## Fix

Measure the variance in an earlier run. Set design.variance.source to record:<sha256> of its estimate record, and design.variance.value to the variance that record holds.

## Why

A variance guessed by hand can be too small, which makes the power gate pass a study that is too small.

## Sources

- <https://doi.org/10.1038/nrn3475>
- <https://doi.org/10.1136/bmj.e5661>

## Applies to

- `design.variance.source` (named:variance-from-record), when mode = confirmatory
