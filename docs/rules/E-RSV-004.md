# E-RSV-004

Severity: error

## Problem

A stratum a claim lists has no reserve input, or a reserve input's stratum is one no claim lists.

## Fix

Give each stratum a claim lists at least one reserve input in that stratum, and list no reserve input in a stratum no claim lists. A claim with no strata takes reserve inputs from every stratum.

## Why

The re-size counts the clusters each claimed stratum needs and takes them from that stratum's reserve. A claimed stratum with no reserve cannot grow, and a reserve that no claim reads holds data no test uses.

## Sources

- <https://www.fda.gov/media/78495/download>

## Applies to

- `design.resize` (named:reserve-strata), when mode = confirmatory
