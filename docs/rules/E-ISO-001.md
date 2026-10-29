# E-ISO-001

Severity: error

## Problem

This machine cannot give the isolation level the plan asks for.

## Fix

Lower scope.min_isolation, or run the plan on a machine that offers the level.

## Why

Running at a lower level than the plan asks for would break the plan's own safety terms.

## Applies to

- `scope.min_isolation` (named:isolation-reachable)
