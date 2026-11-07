# W-MUT-001

Severity: warning

## Problem

Two triggers declare the same child.

## Fix

List the value under one trigger only, or accept that the child runs once and both triggers can admit it.

## Why

A child is its condition with one operator set to one value. Declared twice, it still runs once, so the second declaration adds nothing.

## Applies to

- `mutations.triggers[*].spawns[*].values[*]` (named:child-declared-once)
