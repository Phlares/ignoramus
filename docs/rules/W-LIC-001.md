# W-LIC-001

Severity: warning

## Problem

An input has no licence id.

## Fix

Add the licence the authors gave the data, as an SPDX id. Publishing is blocked until you do.

## Why

Re-sharing data without its licence can break the terms the authors set. The run itself does not need it.

## Sources

- <https://spdx.org/licenses/>

## Applies to

- `inputs[*].licence.spdx` (non-empty)
