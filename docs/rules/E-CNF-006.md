# E-CNF-006

Severity: error

## Problem

A confirmatory study uses a test the checker does not allow.

## Fix

Set design.test to paired-mean.

## Why

v0 confirms only with the paired-mean test. Other tests have no power gate yet.

## Sources


## Applies to

- `design.test` (named:confirmatory-test-allowed), when mode = confirmatory
