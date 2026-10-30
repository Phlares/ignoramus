# E-REF-002

Severity: error

## Problem

A reference names this study.

## Fix

Set estimates.from to the id of another study: one that has run, or will run, before this one.

## Why

A value taken from a study exists only once that study has finished. This study's own values come from this run, so a plan cannot be sized by them.

## Applies to

- `question.hypotheses[*].estimates.from` (named:reference-not-self), when mode = confirmatory
