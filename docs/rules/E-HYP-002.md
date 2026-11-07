# E-HYP-002

Severity: error

## Problem

A stratum a hypothesis lists has no confirmation input, or a stratum a trigger lists has no selection input.

## Fix

List only strata that inputs of the right role name in their stratum field, or add an input for the stratum: confirmation for a hypothesis, selection for a trigger. Leave strata empty to test all units as one group.

## Why

A claim holds only when it holds in every stratum it lists, each tested on its own clusters. A stratum with no input of the role that reads it has no data, so the claim could never hold.

## Sources

- <https://doi.org/10.1080/00401706.1982.10487790>

## Applies to

- `question.hypotheses[*].strata[*]` (named:hypothesis-strata)
- `mutations.triggers[*].strata[*]` (named:hypothesis-strata)
