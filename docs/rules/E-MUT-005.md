# E-MUT-005

Severity: error

## Problem

A spawn value does not fit its operator.

## Fix

List single values of the operator's type: for an enum, a member of its range; for an int, a whole number from lo to hi; for a float, a number from lo to hi.

## Why

A value outside the range, or of another type, would run a child the plan never declared an operator for. A list or a table in place of a value has no meaning as a param.

## Applies to

- `mutations.triggers[*].spawns[*].values[*]` (named:spawn-value-fits)
