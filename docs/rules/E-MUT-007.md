# E-MUT-007

Severity: error

## Problem

The plan has triggers and no selection input.

## Fix

Add an input with role selection, or remove the triggers.

## Why

A trigger decides on selection data, and its children are tested on confirmation data. The tests stay valid only when the set of children that run does not depend on the data they are tested on.

## Sources

- <https://www.fda.gov/media/78495/download>

## Applies to

- `mutations.triggers` (named:triggers-need-selection)
