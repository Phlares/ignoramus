# E-UNQ-001

Severity: error

## Problem

A list repeats an id, path or name.

## Fix

Give every entry its own value. Remove the repeat, or rename it.

## Why

A repeated input would run its units twice and count them twice in the power gate. A stage that lists a condition twice runs it twice. A repeated condition, outcome, measure, hypothesis, operator or trigger id makes later steps pick one entry and ignore the other. Two deviations from one study would record two reasons for one change.

## Applies to

- `design.conditions` (unique)
- `design.selection.conditions` (unique)
- `design.confirmation.conditions` (unique)
- `outcomes` (unique)
- `outcomes` (unique)
- `question.hypotheses` (unique)
- `inputs` (unique)
- `mutations.operators` (unique)
- `mutations.triggers` (unique)
- `pins.deviations` (unique)
