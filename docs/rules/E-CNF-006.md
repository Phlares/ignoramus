# E-CNF-006

Severity: error

## Problem

A confirmatory study has no hypothesis, or a hypothesis of it has no claim.

## Fix

List at least one hypothesis, and give each a claim, { below = m }, { above = m } or { within = [a, b] }, and its statistic. Or make the study exploratory.

## Why

A confirmatory study tests each hypothesis against margins stated before the data come in. A hypothesis with no claim has nothing to test, so any reading of it would be chosen after the fact. A study with no hypothesis would spend a stream slot that can never give a discovery.

## Sources

- <https://doi.org/10.1038/s41562-016-0021>

## Applies to

- `question.hypotheses` (non-empty), when mode = confirmatory
- `question.hypotheses[*].claim` (named:claim-required), when mode = confirmatory
