# E-CNF-002

Severity: error

## Problem

The confirmation sample is too small.

## Fix

Add confirmation clusters or units per cluster until the count reaches the number the power gate needs.

## Why

A test with too few units misses real effects and exaggerates the ones it finds. The gate uses alpha, power, the variance, the smallest effect, and the cluster correlation.

## Sources

- <https://doi.org/10.1038/nrn3475>
- <https://doi.org/10.1136/bmj.e5661>

## Applies to

- `design.confirmation.clusters` (named:power-met), when mode = confirmatory
