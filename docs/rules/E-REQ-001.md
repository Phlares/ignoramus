# E-REQ-001

Severity: error

## Problem

A required field is missing.

## Fix

Add the field. Copy its shape from the template.

## Why

The checker and the run need every field. A silent default would hide a gap in the plan.

## Sources

- <https://toml.io/en/v1.0.0>

## Applies to

- `study` (required)
- `study.id` (required)
- `study.template` (required)
- `study.mode` (required)
- `study.label` (required)
- `study.test_log` (required)
- `question` (required)
- `question.population` (required)
- `question.intervention` (required)
- `question.comparator` (required)
- `question.negative_control` (required)
- `question.hypotheses` (required)
- `question.hypotheses[*].id` (required)
- `question.hypotheses[*].statement` (required)
- `question.hypotheses[*].rival` (required)
- `outcomes` (required)
- `outcomes[*].id` (required)
- `outcomes[*].measure` (required)
- `outcomes[*].paired` (required)
- `outcomes[*].registered` (required)
- `scope` (required)
- `scope.stop_rule` (required)
- `scope.budget` (required)
- `scope.budget.cpu_hours` (required)
- `scope.budget.wall_hours` (required)
- `scope.budget.money_usd` (required)
- `scope.end_of_run` (required)
- `scope.end_of_run.route` (required)
- `scope.end_of_run.follow_on` (required)
- `scope.end_of_run.follow_on[*].when` (required)
- `scope.end_of_run.follow_on[*].spec` (required)
- `design` (required)
- `design.unit` (required)
- `design.cluster` (required)
- `design.stratum` (required)
- `design.reference` (required)
- `design.row_seconds` (required)
- `design.test` (required)
- `design.sesoi` (required)
- `design.icc` (required)
- `design.units_per_cluster` (required)
- `design.variance` (required)
- `design.variance.value` (required)
- `design.variance.source` (required)
- `design.conditions` (required)
- `design.conditions[*].id` (required)
- `inputs` (required)
- `inputs[*].path` (required)
- `inputs[*].sha256` (required)
- `inputs[*].role` (required)
- `inputs[*].licence` (required)
- `inputs[*].licence.spdx` (required)
- `inputs[*].licence.type` (required)
- `inputs[*].licence.source` (required)
- `inputs[*].licence.creators` (required)
- `pins` (required)
- `pins.environment` (required)
- `pins.payload` (required)
- `pins.rule_base` (required)
- `mutations` (required)
- `mutations.operators` (required)
- `mutations.operators[*].name` (required)
- `mutations.operators[*].type` (required)
- `mutations.operators[*].range` (required)
- `mutations.operators[*].acts_on` (required)
- `mutations.triggers` (required)
- `mutations.triggers[*].id` (required)
- `mutations.triggers[*].metric` (required)
- `mutations.triggers[*].threshold_e` (required)
- `mutations.seeds` (required)
- `mutations.seeds.spawn` (required)
- `mutations.seeds.report` (required)
- `report` (required)
- `report.licence_code` (required)
- `report.licence_data` (required)
- `report.publish` (required)
