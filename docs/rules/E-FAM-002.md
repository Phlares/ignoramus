# E-FAM-002

Severity: error

## Problem

A family member cannot be read, is this study or another member again, is already registered, is already locked in a registered family, declares its own family, or lies under another study root.

## Fix

List each member once, by the path of a readable spec under this study root, and not this study itself. Only this study holds a `[family]` table. A registered study cannot join, and a study locked in another family cannot join a second one: give it fresh data, or run it as exploratory on the same data (no claim).

## Why

A family is fixed before any member runs: registering this study locks every member. A registered study may already have seen the data, so a later lock cannot cover it. A study in two families would hold two locks and two reserved slots, and each family's data rules would assume it belongs to that family alone. One table in the first study keeps each family in one place, and one study root lets one payload folder and one data folder serve every member.

## Sources

- <https://arxiv.org/abs/2407.15733>

## Applies to

- `family.members[*]` (named:family-members), when mode = confirmatory
