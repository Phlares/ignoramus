# E-CNF-001

Severity: error

## Problem

A confirmatory hypothesis has no power_at strictly on its claimed side, or power_at lies outside the range of its mean statistic.

## Fix

Set power_at to the true value of the statistic at which the test must have its power: below m for a claim below m, above m for a claim above m, strictly between a and b for a claim within [a, b], and inside [lo, hi] for a mean statistic.

## Why

The sample size depends on the effect the study must detect, so the power gate needs a value to size at. It is set when the plan is written, not read from a pilot: an effect seen in a small pilot is noisy, and sizing at it gives studies too small to detect the effect.

## Sources

- <https://doi.org/10.1016/j.jesp.2017.09.004>
- <https://doi.org/10.1038/nrn3475>

## Applies to

- `question.hypotheses[*].power_at` (named:power-at-side), when mode = confirmatory
