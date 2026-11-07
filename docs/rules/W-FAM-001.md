# W-FAM-001

Severity: warning

## Problem

A family member's value waits on a study that has not finished.

## Fix

Nothing to do now: the member reads the value when it registers, after that study ends. If the member then fails a gate, it is refused and its reserved stream slot stays unused, so size its reserve inputs generously.

## Why

A member's values may come from a study that runs first, such as a pilot or an earlier member. They are read at the member's own registration and cannot change after it. This study's check cannot run the member's gates on values that do not exist yet, so it lists each such value and the study it waits on.

## Applies to

- the command line, not a spec field
