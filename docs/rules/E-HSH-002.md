# E-HSH-002

Severity: error

## Problem

`pins.rule_base` is set and is not the rules hash of this box.

## Fix

Run `ignorarium check` and copy the `rules` hash it prints into `pins.rule_base`, or leave `pins.rule_base` empty.

## Why

The pin says which rules the plan was approved under. A box with other rules judges a different study.

## Applies to

- `pins.rule_base` (named:rule-base-matches)
