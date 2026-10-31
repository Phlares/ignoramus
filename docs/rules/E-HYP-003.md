# E-HYP-003

Severity: error

## Problem

A claim or a statistic is not well formed.

## Fix

Write the claim as one of { below = m }, { above = m } or { within = [a, b] } with a below b. Give a claim one statistic: { mean = [lo, hi] } with lo below hi and every margin strictly between them, or { quantile = q } with q strictly between 0 and 1.

## Why

A claim is one or two one-sided bounds, and it holds when every bound holds. within [a, b] is two one-sided tests, above a and below b, so it needs no alpha correction. The test counts each cluster once: the mean of its units, or the share of its units on the claimed side. Before a mean is taken, each unit is clipped to the declared range, so every cluster mean lies inside it and the claim is about the mean of the clipped units. A unit far outside the range cannot then push a cluster mean outside it, which the test could not bound. A margin at or outside the range can never be tested.

## Sources

- <https://doi.org/10.1080/00401706.1982.10487790>
- <https://doi.org/10.1177/1948550617697177>

## Applies to

- `question.hypotheses[*].claim` (named:hypothesis-shape)
- `question.hypotheses[*].statistic` (named:hypothesis-shape)
