# E-RSV-001

Severity: error

## Problem

A reserve input is listed, and design.resize declares no re-size.

## Fix

Declare the re-size: add [design.resize] with rule = "blinded-once". Or give the input another role.

## Why

A reserve input holds confirmation clusters that run only when the re-size asks for them. Without a declared re-size, nothing fixed before the data decides whether they run, so the choice could follow the results.

## Sources

- <https://www.fda.gov/media/78495/download>

## Applies to

- `inputs[*].role` (named:reserve-declared), when mode = confirmatory
