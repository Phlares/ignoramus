# E-REF-001

Severity: error

## Problem

design.conditions is empty, or design.reference is not one of its ids.

## Fix

List at least one condition in design.conditions. Set design.reference to the id of one of them.

## Why

Every comparison is made against the reference condition. A reference that matches no condition leaves the run with nothing to compare to, and the registered id cannot be reused.

## Applies to

- `design.conditions` (non-empty)
- `design.reference` (named:reference-is-condition)
