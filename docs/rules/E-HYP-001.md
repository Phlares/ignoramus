# E-HYP-001

Severity: error

## Problem

A hypothesis names an outcome that is not listed, or its contrast is empty, has a weight of 0, or names a condition the confirmation stage does not run.

## Fix

Set outcome to an id from outcomes. Give contrast one or more condition ids from design.confirmation.conditions, each with a weight other than 0, for example { selected = 1.0 } or { arm-a = 1.0, arm-b = -1.0 }. Name selected only with a selection stage, and never the reference.

## Why

The test reads the contrast's paired values in the confirmation stage. A condition that stage does not run, or the reference, which has no paired value, leaves every unit incomplete, and the claim fails after the budget is spent.

## Applies to

- `question.hypotheses[*].outcome` (named:hypothesis-shape)
- `question.hypotheses[*].contrast` (named:hypothesis-shape)
- `question.hypotheses[*].contrast.*` (named:hypothesis-shape)
- `question.hypotheses[*].contrast.*` (named:condition-declared)
