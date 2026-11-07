# E-MUT-001

Severity: error

## Problem

The spawn depth cap is above 2, or below 1 in a plan with a declared trigger.

## Fix

Set mutations.depth_cap to 2 or less, and to 1 or more when the plan declares a trigger.

## Why

Each spawn level multiplies the number of runs and the chances of a chance finding. The cap is 2. A declared trigger spawns children at level 1, so a cap of 0 would contradict the plan.

## Applies to

- `mutations.depth_cap` (named:depth-cap)
