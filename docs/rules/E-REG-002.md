# E-REG-002

Severity: error

## Problem

A study with this id is already registered in the box.

## Fix

Choose a new `study.id` for a new plan. A registered plan cannot be replaced.

## Why

A plan registered once is fixed. A second plan under the same id would hide the first.

## Applies to

- the command line, not a spec field
