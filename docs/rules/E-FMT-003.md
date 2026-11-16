# E-FMT-003

Severity: error

## Problem

An input hash or a record hash is not a SHA-256 digest.

## Fix

Use 64 lowercase hex characters, with no sha256: prefix. An input hash may stay empty until the input is pinned; study.replicates.done may not.

## Why

The run compares this hash with the file it reads. Any other form can never match.

## Sources

- <https://toml.io/en/v1.0.0>

## Applies to

- `inputs[*].sha256` (format)
- `study.replicates.done` (format)
