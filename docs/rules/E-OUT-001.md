# E-OUT-001

Severity: error

## Problem

An outcome has no unit.

## Fix

Give every outcome a unit, for example ratio or seconds.

## Why

An effect size has no meaning without its unit. Margins and power_at are stated in this unit.

## Sources

- <https://doi.org/10.1136/bmj.e7586>

## Applies to

- `outcomes[*].unit` (non-empty)
