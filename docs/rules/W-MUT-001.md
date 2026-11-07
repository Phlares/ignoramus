# W-MUT-001

Severity: warning

## Problem

A child is declared twice.

## Fix

List each value once, under one trigger. Equal numbers, such as 64 and 64.0, are one value. Otherwise accept that the child runs once and the first trigger that fires admits it.

## Why

A child is its condition with one operator set to one value, and its id names the three. Declared twice, it still runs once and its cost counts once, so the second declaration adds nothing.

## Applies to

- `mutations.triggers[*].spawns[*].values[*]` (named:child-declared-once)
