# E-ENM-001

Severity: error

## Problem

The value is not one of the allowed values.

## Fix

Use one of the values listed in the template comment for this field.

## Why

A value outside the set has no defined meaning in the checker or the run.

## Sources

- <https://toml.io/en/v1.0.0>

## Applies to

- `study.template` (enum)
- `study.mode` (enum)
- `study.label` (enum)
- `scope.stop_rule` (enum)
- `scope.end_of_run.route` (enum)
- `design.pick` (enum)
- `design.selection.rule` (enum)
- `outcomes[*].paired` (enum)
- `inputs[*].role` (enum)
- `inputs[*].licence.type` (enum)
- `mutations.operators[*].type` (enum)
- `report.publish` (enum)
- `pins.deviations[*].timing` (enum)
