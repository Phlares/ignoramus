# E-HSH-001

Severity: error

## Problem

An input file is missing or its SHA-256 differs from the pin.

## Fix

Fix the path, or update the sha256 if the file was changed on purpose.

## Why

The pin says which bytes the plan was approved on. A different file is a different study.

## Sources


## Applies to

- `inputs[*].sha256` (named:input-hash-matches)
