# E-CNF-002

Severity: error

## Problem

The confirmation sample is too small.

## Fix

Add confirmation inputs to each group the detail names, until it has the clusters the power gate needs. Raising design.units_per_cluster helps only as far as the icc allows, since the test takes one value per cluster. Two inputs that hold the same data are one cluster.

## Why

A test with too few clusters misses real effects and exaggerates the ones it finds. The test takes one value per cluster, so the gate counts the clusters of each stratum the claim lists, or of all confirmation inputs when it lists none. For a mean statistic it uses the hypothesis's share of design.alpha, design.power, the sd and icc from the study in estimates, the harmonic mean of the units its clusters give, and the distance from power_at to each margin. The gate assumes the box estimates the statistic as the unweighted mean of the cluster means: one small cluster then weighs as much as a large one, which the harmonic mean accounts for.

## Sources

- <https://doi.org/10.1038/nrn3475>
- <https://doi.org/10.1136/bmj.e5661>

## Applies to

- `question.hypotheses[*]` (named:power-met), when mode = confirmatory
