# E-MUT-003

Severity: error

## Problem

A trigger names an outcome that is not listed.

## Fix

Set the trigger outcome to the id of an outcome. Declare the trigger with outcome, contrast, claim, statistic, threshold_e and spawns.

## Why

A trigger on an unregistered outcome would let the run pick its own signal after the fact.

## Applies to

- `mutations.triggers[*].outcome` (named:trigger-declared)
