# E-RSV-004

Severity: error

## Problem

A stratum a claim lists has no reserve input, a reserve input's stratum is one no claim lists, or a claim with no strata has reserve inputs in more than one stratum.

## Fix

Give each stratum a claim lists at least one reserve input in that stratum, and list no reserve input in a stratum no claim lists. A claim with no strata takes every reserve input, so keep them all in one stratum, or list the strata in the claim.

## Why

The re-size counts the clusters each claimed stratum needs and takes them from that stratum's reserve. A claimed stratum with no reserve cannot grow, and a reserve that no claim reads holds data no test uses. A claim with no strata takes reserve clusters one stratum after another, so a count smaller than the reserve would add only the first stratum's clusters and shift the pooled mean toward that stratum.

## Sources

- <https://www.fda.gov/media/78495/download>

## Applies to

- `design.resize` (named:reserve-strata), when mode = confirmatory
