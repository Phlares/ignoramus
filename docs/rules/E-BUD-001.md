# E-BUD-001

Severity: error

## Problem

The budget is empty.

## Fix

Set at least one of cpu_hours, wall_hours or money_usd above 0.

## Why

A run with no budget has no stop. The budget is the cap the run enforces.

## Sources


## Applies to

- `scope.budget` (named:budget-present)
