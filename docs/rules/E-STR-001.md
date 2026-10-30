# E-STR-001

Severity: error

## Problem

The stream gives this confirmatory study no level.

## Fix

Run the plan as an exploratory study, or set study.test_log = true to try it against the test stream, which never counts and never runs out. The plan stream gives levels to its first 10 confirmatory studies only. A stream log that cannot be read, or that lacks a slot a register record names, must be restored from a copy that holds every slot before any confirmatory study registers.

## Why

A confirmatory study takes its level from the stream of the log it registers in, never from the plan: e-LOND with alpha 0.05 split evenly over 10 slots. A slot's level is 0.005 times one more than the discoveries closed before it opened, and a slot is a discovery when the study's e-value reaches 1 / level. When each study's e-value has expected value at most 1 under its null hypothesis, as the box's fixed-size tests give, the false discovery rate over every claim the stream confirms stays at or below 0.05, whatever the dependence between the studies. The slots go in the order studies register, which is fixed before any result. The plan stream never resets and refuses a study beyond its 10th slot, so results never stretch the budget to more studies. The test stream starts a new epoch, a fresh stream of 10 slots, each time its slots are used up, because its studies never count. Each slot records its level, its e-value and the decision. Every slot a register record names must be in the stream, so a stream log that was cut or rolled back is refused, not read as a fresh budget. register opens the slot and refuses when the level has dropped below the one check showed.

## Sources

- <https://arxiv.org/abs/2311.06412>
- <https://arxiv.org/abs/2208.11418>
- <https://doi.org/10.1038/s41562-017-0189-z>

## Applies to

- `study.mode` (named:stream-level), when mode = confirmatory
