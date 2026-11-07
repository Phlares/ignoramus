# E-FAM-003

Severity: error

## Problem

An input of a family member is not pinned by sha256, or its data is neither at the study root nor in the store; or a selection input of this study has no data at the study root or in the store.

## Fix

Set sha256 on every input of every member, of every role, and put each file at the study root or in the store before this study registers. Put this study's own selection files there too. Member specs are final then: nothing about a member can change after the lock.

## Why

Registering this study locks every member, inputs included. The lock needs the bytes: the box compares member data with earlier uses and with selection data by content, and an absent file has no content to compare. A member may not confirm on this study's selection data, so that data must be present too when the family is checked.

## Sources

- <https://arxiv.org/abs/2407.15733>

## Applies to

- `family` (named:family-pinned), when mode = confirmatory
- `family.members[*]` (named:family-pinned), when mode = confirmatory
