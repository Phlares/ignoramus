# W-MUT-002

Severity: warning

## Problem

Running every declared child would exceed the budget.

## Fix

Raise scope.budget, or declare fewer children. Otherwise a run where the triggers fire skips each child that does not fit, in declared order.

## Why

A fired trigger admits a child only when the spec estimate of the plan's rows at design.row_seconds, with the children admitted before it and its own rows, stays within each cap of scope.budget. The spec decides, not measured time, so every machine admits the same children and check can name the children a run would skip. check raises this warning; the detail names those children.

## Applies to

- the command line, not a spec field
