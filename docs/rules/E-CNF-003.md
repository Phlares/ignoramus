# E-CNF-003

Severity: error

## Problem

The variance is not taken from a record.

## Fix

Measure the variance in an earlier run and set design.variance.source to record:<sha256> of that record.

## Why

A variance guessed by hand can be too small, which makes the power gate pass a study that is too small.

## Sources

- <https://doi.org/10.1038/nrn3475>
- <https://doi.org/10.1136/bmj.e5661>

## Applies to

- `design.variance.source` (named:variance-from-record), when mode = confirmatory
