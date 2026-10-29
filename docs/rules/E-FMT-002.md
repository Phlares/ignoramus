# E-FMT-002

Severity: error

## Problem

The variance source is not in a known form.

## Fix

Use stated, or record:<sha256> with the 64 lowercase hex characters of a record.

## Why

A variance must say where it came from. A record reference must name one exact record.

## Sources

- <https://toml.io/en/v1.0.0>

## Applies to

- `design.variance.source` (format)
