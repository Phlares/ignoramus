# E-RSV-002

Severity: error

## Problem

design.resize declares a re-size, and no input is a reserve.

## Fix

List the clusters the re-size may add as inputs with role = "reserve", in the strata the claims list. Or remove [design.resize].

## Why

The re-size runs once, after the confirmation clusters, and counts how many reserve clusters each claimed stratum adds. With no reserve it can add nothing. The count reads the root mean square of the first clusters' observations about 0 and no condition's name. It computes no mean, but the square carries the squared effect, so the count shows a little of the effect. The e-value test keeps its level whatever count the re-size picks, because each bet uses only the observations before it.

## Sources

- <https://www.fda.gov/media/78495/download>
- <https://doi.org/10.1002/sim.1585>

## Applies to

- `design.resize` (named:reserve-declared), when mode = confirmatory
