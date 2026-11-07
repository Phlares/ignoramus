# E-MUT-003

Severity: error

## Problem

A trigger has the old shape, or names an outcome that is not listed.

## Fix

Declare each trigger with outcome, contrast, claim, statistic, threshold_e and spawns, in place of metric. Set outcome to the id of an outcome.

## Why

A trigger on an unregistered outcome would let the run pick its own signal after the fact. The old shape names a metric and no claim, so nothing fixes what the trigger bets on or what it spawns.

## Applies to

- `mutations.triggers[*]` (named:trigger-declared)
- `mutations.triggers[*].outcome` (named:trigger-declared)
