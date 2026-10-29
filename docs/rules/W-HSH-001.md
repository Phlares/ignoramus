# W-HSH-001

Severity: warning

## Problem

An input has no SHA-256 pin.

## Fix

Pin the input: add its SHA-256, 64 lowercase hex characters.

## Why

Without a pin the run cannot tell if the file changed after the plan was approved. The run itself still starts.

## Sources


## Applies to

- `inputs[*].sha256` (non-empty)
