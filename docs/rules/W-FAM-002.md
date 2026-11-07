# W-FAM-002

Severity: warning

## Problem

A confirmatory spec reachable through follow-on routes shares confirmation or reserve data with this study and is not in its family.

## Fix

To keep both claims, list that spec in this study's `[family]` before this study registers. Cost: every member is locked and takes a plan stream slot at register. Otherwise give one of the two studies fresh data, or run one as exploratory (no claim).

## Why

The later study cannot confirm on data this study used, unless both belong to one family. Once this study registers, the family option is closed. The warning shows it while it can still be taken.

## Sources

- <https://arxiv.org/abs/2311.06412>

## Applies to

- `scope.end_of_run.follow_on[*].spec` (named:follow-on-shares), when mode = confirmatory
