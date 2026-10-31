# E-END-001

Severity: error

## Problem

The end-of-run route is not valid.

## Fix

For route follow-on, list at least one follow_on entry. Write each when as done, which runs the follow-on whatever the result, or as <hypothesis id>.pass or <hypothesis id>.fail, using a hypothesis that has a claim.

## Why

A follow-on whose condition can never be met never runs. A hypothesis with no claim has no pass or fail: a confirmatory claim passes on its test, an exploratory one on its point estimate. The route must say what happens, before the data come in.

## Applies to

- `scope.end_of_run.follow_on` (named:route-valid)
- `scope.end_of_run.follow_on[*].when` (named:route-valid)
