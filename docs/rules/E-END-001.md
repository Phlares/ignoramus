# E-END-001

Severity: error

## Problem

The end-of-run route is not valid.

## Fix

For route follow-on, list at least one follow_on entry. Write each when as <hypothesis id>.pass or <hypothesis id>.fail, using an id from question.hypotheses.

## Why

A follow-on that names no hypothesis can never run. The route must say what happens, before the data come in.

## Sources


## Applies to

- `scope.end_of_run.follow_on` (named:route-valid)
- `scope.end_of_run.follow_on[*].when` (named:route-valid)
