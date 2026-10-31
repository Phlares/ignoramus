# E-CNF-004

Severity: error

## Problem

The plan has no confirmation input, or a confirmation input holds the same data as a selection or calibration input.

## Fix

Add a confirmation input. Give each one data that no selection or calibration input holds: not the same bytes, and not the same text in other case or spacing.

## Why

A winner picked on some data looks better on that same data. Only fresh data gives an honest test. The content decides, not the file name. A model calibrated on a text has adapted to it; its error there is not its error on new text.

## Sources

- <https://doi.org/10.1038/s41562-016-0021>
- <https://arxiv.org/html/2311.09755v2>

## Applies to

- `inputs` (named:confirmation-disjoint), when mode = confirmatory
- `inputs[*].sha256` (named:confirmation-disjoint), when mode = confirmatory
