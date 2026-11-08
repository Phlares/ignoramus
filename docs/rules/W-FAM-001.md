# W-FAM-001

Severity: warning

## Problem

A family member's value waits on a study that has not finished.

## Fix

Check shows this as a warning. Register the root after the study finishes: `register` refuses a root whose member waits on a study outside the family. A wait on the root or on another member of the family does not refuse, because it can end only after the root registers.

## Why

A member's values may come from a study that runs first, such as a pilot or an earlier member. This study's check cannot run the member's gates on values that do not exist yet, so it lists each such value and the study it waits on. The lock must not hold a member that cannot pass its gates, so the root registers only after every outside study its members borrow from has finished.

## Applies to

- the command line, not a spec field
