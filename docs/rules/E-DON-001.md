# E-DON-001

Severity: error

## Problem

The study is done.

## Fix

Read it with `report`, `status` or `verify`. Start new work as a new study.

## Why

A signed `done` record ends the study and states how and when. A row, a session or a stop after it would change a study whose end is already fixed, so `run`, `resume`, `stop` and `pause` refuse before they write anything.

## Applies to

- the command line, not a spec field
