# E-RNG-001

Severity: error

## Problem

A number is outside its allowed range.

## Fix

Bring the value into range: min_isolation 0 to 3, alpha above 0 up to 0.5, power 0.5 to 0.999, repeats at least 1, each budget value 0 or more, row_seconds above 0, units_per_cluster 1 up to 4294967295, each input's units 1 up to 4294967295, repeats up to 4294967295, depth_cap 0 to 255, each trigger threshold_e above 0.

## Why

Values outside these ranges are meaningless or make the sample-size maths fail. The default alpha is 0.005, because 0.05 gives too many false findings.

## Sources

- <https://doi.org/10.1038/s41562-017-0189-z>
- <https://doi.org/10.1038/nrn3475>
- <https://doi.org/10.1136/bmj.e5661>

## Applies to

- `scope.min_isolation` (range)
- `design.alpha` (range)
- `design.power` (range)
- `design.repeats` (range)
- `scope.budget.cpu_hours` (range)
- `scope.budget.wall_hours` (range)
- `scope.budget.money_usd` (range)
- `design.row_seconds` (range)
- `design.units_per_cluster` (range)
- `inputs[*].units` (range)
- `mutations.depth_cap` (range)
- `mutations.triggers[*].threshold_e` (range)
