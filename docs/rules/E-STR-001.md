# E-STR-001

Severity: error

## Problem

The stream gives this confirmatory study no level.

## Fix

Run the plan as an exploratory study, or set study.test_log = true to try it against the test stream, which never counts. A stream gives levels to its first 10 confirmatory studies only. A stream log that cannot be read must be restored before any confirmatory study registers.

## Why

A confirmatory study takes its level from the stream of the log it registers in, never from the plan: e-LOND with alpha 0.05 split evenly over 10 slots. A slot's level is 0.005 times one more than the discoveries closed before it opened, and a slot is a discovery when the study's e-value reaches 1 / level. The false discovery rate over every claim the stream confirms then stays at or below 0.05 under any dependence between the studies. The slots go in the order studies register, which is fixed before any result, and the stream refuses a study beyond its 10th slot, so results never stretch the budget to more studies. Each slot records its level, its e-value and the decision. register opens the slot and refuses when the level has dropped below the one check showed.

## Sources

- <https://arxiv.org/abs/2311.06412>
- <https://arxiv.org/abs/2208.11418>
- <https://doi.org/10.1038/s41562-017-0189-z>

## Applies to

- `study.mode` (named:stream-level), when mode = confirmatory
