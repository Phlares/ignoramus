# E-STP-004

Severity: error

## Problem

The head line of the stamped file names a record the log does not hold with that hash.

## Fix

Stamp a file `stamp --request` wrote from this study's log. Check the log with `verify --study <id>`.

## Why

The head line says the stamp covers the records up to that one. The chain makes that true only when the log holds that record with that hash.

## Applies to

- the command line, not a spec field
