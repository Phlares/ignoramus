# E-UNT-002

Severity: error

## Problem

The schedule has more rows than the box allows.

## Fix

Lower design.units_per_cluster, design.repeats, the number of conditions a stage runs, or the units of the selection and confirmation inputs.

## Why

check builds the whole schedule to count its rows and cost. A typo in a unit count must stop here, not exhaust the memory.

## Applies to

- `design.units_per_cluster` (named:row-cap)
