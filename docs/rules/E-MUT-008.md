# E-MUT-008

Severity: error

## Problem

A confirmatory plan has triggers.

## Fix

Run the study as exploratory, or remove the triggers.

## Why

Children chosen by a trigger may inflate the error rate of a confirmatory claim. This holds until the box simulates the declared spawn policy and evaluates the trigger decision blind. An exploratory study tests no claim, so its children cost nothing in level.

## Sources

- <https://www.fda.gov/media/78495/download>

## Applies to

- `mutations.triggers` (named:triggers-declared), when mode = confirmatory
