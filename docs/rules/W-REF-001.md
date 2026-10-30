# W-REF-001

Severity: warning

## Problem

A value waits on a study that has not finished.

## Fix

Run the study it names to its end, then check again. Until then check exits 3 and register refuses the plan; nothing else needs to change.

## Why

A follow-on plan is written and checked before the study it builds on runs. Its values are read from that study's records once the study has written them all, so a value cannot change after the plan is registered. The box cannot read a study it has not run.

## Applies to

- `question.hypotheses[*].estimates` (named:reference-finished), when mode = confirmatory
