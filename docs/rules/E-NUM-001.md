# E-NUM-001

Severity: error

## Problem

A number is not finite (nan or inf).

## Fix

Replace nan or inf with a finite value.

## Why

TOML allows nan and inf, but JSON does not. The spec hash and the sample-size maths need finite numbers.

## Sources

- <https://www.rfc-editor.org/rfc/rfc8259#section-6>
- <https://toml.io/en/v1.0.0#float>

## Applies to

- `**` (finite)
