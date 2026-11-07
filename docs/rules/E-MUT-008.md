# E-MUT-008

Severity: error

## Problem

A confirmatory plan has triggers.

## Fix

Run the study as exploratory, or remove the triggers.

## Why

Children chosen after a look at the data inflate the error rate of a confirmatory claim, and the simulation that would show the size of that inflation is not built. An exploratory study tests no claim, so its children cost nothing in level.

## Sources

- <https://www.fda.gov/media/78495/download>

## Applies to

- `mutations.triggers` (named:triggers-declared), when mode = confirmatory
