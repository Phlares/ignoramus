# E-REF-002

Severity: error

## Problem

A reference names this study, or a study that recorded no selection.

## Fix

Set estimates.from or a param's from to the id of another study: one that has run, or will run, before this one. A param takes the condition that study selected, so that study needs a selection stage.

## Why

A value taken from a study exists only once that study has finished. This study's own values come from this run, so a plan cannot be sized by them or take a condition from them. A study without a selection stage selects no condition.

## Applies to

- `question.hypotheses[*].estimates.from` (named:reference-not-self)
- `design.conditions[*].params.*.from` (named:reference-not-self)
