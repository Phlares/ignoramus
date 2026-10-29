# E-MUT-003

Severity: error

## Problem

A trigger names a metric that is not an outcome.

## Fix

Set the trigger metric to the id of an outcome.

## Why

A trigger on an unregistered metric would let the run pick its own signal after the fact.

## Sources


## Applies to

- `mutations.triggers[*].metric` (named:trigger-declared)
