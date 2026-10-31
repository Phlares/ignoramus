# E-UNT-001

Severity: error

## Problem

The plan has no selection or confirmation input, or one does not declare its units, or its stratum while the design names strata.

## Fix

List each file of units as a selection or confirmation input. Set its units to the number of units the file holds. When design.stratum is set, give each such input its stratum.

## Why

Each selection or confirmation input is one cluster, and the schedule picks its units from the count the input declares. The box does not count units itself. A plan without a cluster runs nothing.

## Applies to

- `inputs` (named:units-declared)
- `inputs[*].units` (named:units-declared)
- `inputs[*].stratum` (named:units-declared)
