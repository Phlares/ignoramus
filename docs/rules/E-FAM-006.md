# E-FAM-006

Severity: error

## Problem

This study is a member of a registered family and differs from what the family locked.

## Fix

Choose one: restore the spec, payload or input the family locked, byte for byte; or register the changed study under a new study id, as a study outside the family. Outside the family the freshness rule applies in full: it needs confirmation and reserve data no earlier study used, or it runs as exploratory on the same data (no claim).

## Why

Members may share data because every member was fixed before any of them ran. A member changed after the lock may have been shaped by what the root or another member found, so a test of it on the shared data is no longer an independent test. The lock holds each member's spec hash, payload hash and input pins, so any change shows. `check` reports this rule for confirmatory plans; `register` refuses a changed member in any mode, so no change of mode frees a locked id or its reserved slot.

## Sources

- <https://doi.org/10.1038/s41562-016-0021>
- <https://journalofcognition.org/about/registered-reports>

## Applies to

- `study.id` (named:family-lock), when mode = confirmatory
