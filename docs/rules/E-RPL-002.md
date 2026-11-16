# E-RPL-002

Severity: error

## Problem

The original's verdict for the hypothesis is not a pass on its test.

## Fix

Replicate a hypothesis whose registered test passed in the original: its verdict record says pass, on basis test. A claim that passed on its point estimate comes from an exploratory study; test it in an ordinary confirmatory study, with no study.replicates.

## Why

A replication tests a result that a registered test already showed. The criterion is the two-trials rule: the replication succeeds when its own registered test passes, given that the original's test passed. Two results that both fail to pass are not a replication, and a pass on a point estimate was never tested.

## Applies to

- `study.replicates` (named:replicates-passed)
