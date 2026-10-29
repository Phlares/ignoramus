# E-KEY-001

Severity: error

## Problem

The spec has a field the schema does not define.

## Fix

Remove the field, or fix its name. The template lists every field.

## Why

A misspelt field is silently ignored by most readers. Rejecting it keeps the plan and the run in step.

## Sources

- <https://toml.io/en/v1.0.0>

## Applies to

- `**` (known-key)
