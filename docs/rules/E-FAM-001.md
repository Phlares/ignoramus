# E-FAM-001

Severity: error

## Problem

An exploratory study declares a family, or a family member is not confirmatory or uses another log than this study.

## Fix

Make this study and every member confirmatory, with the same study.test_log. An exploratory study needs no family to reuse data: remove the `[family]` table, or remove the member from it and run it as exploratory on the same data (no claim).

## Why

Members may share data because each takes its own slot in one stream, and the stream keeps the false discovery rate at or below 0.05 over its studies whatever the dependence between them, shared data included. A study in another stream, or with no slot, falls outside that bound. An exploratory study makes no claim, so it needs no family.

## Sources

- <https://arxiv.org/abs/2311.06412>

## Applies to

- `family` (named:family-confirmatory)
- `family.members[*]` (named:family-confirmatory)
