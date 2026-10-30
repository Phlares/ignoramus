# E-SEL-001

Severity: error

## Problem

A stage is not well formed: a selection stage with no selection input, or a selection input with no stage; fewer than two candidates; a stage condition that is the reference or not in design.conditions; a metric that is not an outcome; selected with no selection stage; or a condition named selected.

## Fix

Give design.selection two or more candidates from design.conditions other than the reference, a metric that is an outcome id, and at least one selection input; or remove the stage and the selection inputs. List conditions from design.conditions in design.confirmation.conditions, and selected only with a selection stage. Name no condition selected.

## Why

The selection stage picks one candidate on the selection inputs by a rule fixed before the data come in; the confirmation stage then tests the pick on other data. Only a declared rule over declared candidates shows that the confirmation data did not shape the choice. The reference runs in every stage.

## Sources

- <https://www.fda.gov/media/78495/download>

## Applies to

- `design.selection` (named:selection-stage)
- `inputs[*].role` (named:selection-stage)
- `design.selection.conditions` (named:selection-stage)
- `design.selection.conditions[*]` (named:selection-stage)
- `design.selection.metric` (named:selection-stage)
- `design.confirmation.conditions[*]` (named:selection-stage)
- `design.conditions[*].id` (named:selection-stage)
