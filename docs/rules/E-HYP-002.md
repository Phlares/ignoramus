# E-HYP-002

Severity: error

## Problem

A stratum a hypothesis lists has no confirmation input.

## Fix

List only strata that confirmation inputs name in their stratum field, or add a confirmation input for the stratum. Leave strata empty to test all units as one group.

## Why

A claim holds only when it holds in every stratum it lists, each tested on its own clusters. A stratum with no confirmation input has no data, so the claim could never hold.

## Sources

- <https://doi.org/10.1080/00401706.1982.10487790>

## Applies to

- `question.hypotheses[*].strata[*]` (named:hypothesis-strata)
