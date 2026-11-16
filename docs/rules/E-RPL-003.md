# E-RPL-003

Severity: error

## Problem

This study is not confirmatory, or has no hypothesis with a claim under the original's hypothesis id.

## Fix

Make the study confirmatory, and give it a hypothesis with the id that study.replicates.hypothesis names, with a claim. The original and the replication test one hypothesis under one id.

## Why

The criterion reads the replication's own verdict for the hypothesis. Only a confirmatory hypothesis with a claim gets its verdict from a registered test.

## Applies to

- `study.replicates` (named:replicates-confirmatory)
