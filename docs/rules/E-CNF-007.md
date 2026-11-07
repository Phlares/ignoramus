# E-CNF-007

Severity: error

## Problem

A confirmation or reserve input holds the selection data of a study that a param takes its condition from.

## Fix

Give this study confirmation and reserve data that the selection inputs of that study did not hold: not the same bytes, and not the same content in other case or spacing. Or run this study as exploratory on the same data (no claim).

## Why

A param { from = "<study id>" } takes the condition that study picked on its selection data. A condition picked on some data looks better on that same data, so a test of it there is not an honest test. Its evidence must come from other data. This holds for every confirmatory study, family members included: a member locked before the pick still takes the pick. The box reads the selection data from the inputs the study froze at its first run, and compares content, not file names. A study that has not finished is checked when this study registers, since register waits for it.

## Sources

- <https://www.fda.gov/media/78495/download>

## Applies to

- `design.conditions[*].params.*` (named:from-selection), when mode = confirmatory
