# E-CNF-001

Severity: error

## Problem

A confirmatory study has no smallest effect of interest above zero.

## Fix

Set design.sesoi to the smallest effect, in outcome units, that you would act on. It must be above 0.

## Why

The sample size depends on the effect you care about. Without it the power gate has nothing to size.

## Sources

- <https://doi.org/10.1038/nrn3475>
- <https://doi.org/10.1136/bmj.e5661>

## Applies to

- `design.sesoi` (named:sesoi-required), when mode = confirmatory
