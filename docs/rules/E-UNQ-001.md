# E-UNQ-001

Severity: error

## Problem

A list repeats an id, path or name.

## Fix

Give every entry its own value. Remove the repeat, or rename it.

## Why

A repeated cluster would count its units twice in the power gate. A repeated condition, outcome, hypothesis, input, operator or trigger id makes later steps pick one entry and ignore the other.

## Applies to

- `design.selection.clusters` (unique)
- `design.confirmation.clusters` (unique)
- `design.conditions` (unique)
- `outcomes` (unique)
- `question.hypotheses` (unique)
- `inputs` (unique)
- `mutations.operators` (unique)
- `mutations.triggers` (unique)
