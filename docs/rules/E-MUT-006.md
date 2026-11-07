# E-MUT-006

Severity: error

## Problem

An operator acts on a condition that is not declared.

## Fix

Set acts_on to the id of a condition in design.conditions, or to selected when the plan has a design.selection stage.

## Why

A child takes the params of the condition its operator acts on. With no such condition, or no pick for selected, the child has no params to start from.

## Applies to

- `mutations.operators[*].acts_on` (named:acts-on-declared)
