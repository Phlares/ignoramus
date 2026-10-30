# E-CNF-004

Severity: error

## Problem

The plan has no confirmation input, or a confirmation input holds the same data as a selection input.

## Fix

Add a confirmation input. Give each one data that no selection input holds: not the same bytes, and not the same text in other case or spacing.

## Why

A winner picked on some data looks better on that same data. Only fresh data gives an honest test. The content decides, not the file name.

## Sources

- <https://doi.org/10.1038/s41562-016-0021>

## Applies to

- `inputs` (named:selection-confirmation-disjoint), when mode = confirmatory
- `inputs[*].sha256` (named:selection-confirmation-disjoint), when mode = confirmatory
