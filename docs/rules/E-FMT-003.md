# E-FMT-003

Severity: error

## Problem

An input hash is not a SHA-256 digest.

## Fix

Use 64 lowercase hex characters, or leave it empty until the input is pinned.

## Why

The run compares this hash with the file it reads. Any other form can never match.

## Sources

- <https://toml.io/en/v1.0.0>

## Applies to

- `inputs[*].sha256` (format)
