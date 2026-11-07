# E-MUT-004

Severity: error

## Problem

A trigger spawns an operator that is not declared.

## Fix

Set each spawns operator to the name of an entry in mutations.operators, or declare the operator.

## Why

A child is its condition with one operator set to one value. An operator the plan does not declare has no type, no range and no condition to act on.

## Applies to

- `mutations.triggers[*].spawns[*].operator` (named:spawn-operator-declared)
