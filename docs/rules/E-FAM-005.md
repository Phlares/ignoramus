# E-FAM-005

Severity: error

## Problem

The plan stream has no free slot for every member of this family.

## Fix

Choose one: list fewer members; put this study and every member on the test stream (study.test_log = true), which never runs out and never counts; or run them as exploratory studies (no claim). The plan stream gives levels to 10 confirmatory studies, and a slot reserved for a member of a registered family stays reserved.

## Why

Registering this study locks every member and reserves a plan stream slot for each, so no later study can take the slot a member needs. A member locked with no slot could never make its claim, and the data locked for it would be spent for nothing. The check counts the slot this study opens, one slot per member, and the slots already reserved for members of other families that have not registered yet.

## Sources

- <https://arxiv.org/abs/2311.06412>

## Applies to

- `family` (named:family-room), when mode = confirmatory
