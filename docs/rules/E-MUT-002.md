# E-MUT-002

Severity: error

## Problem

The spawn and report seed sets overlap.

## Fix

Give spawn and report seed prefixes where neither is a prefix of the other, for example spawn/ and report/.

## Why

Overlapping seeds would let spawned runs reuse the draws that the report runs use.

## Sources


## Applies to

- `mutations.seeds` (named:seed-sets-disjoint)
