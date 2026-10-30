# E-REG-003

Severity: error

## Problem

The study root has no usable `payload/` folder, or `pins.payload` is not its hash.

## Fix

Put the payload in `payload/` under the study root: an executable `payload/run`, and only regular files and folders below it, no links. Leave `pins.payload` empty, or set it to the folder hash that `register` prints.

## Why

The box copies the payload folder at registration and every row runs that copy. A copy of other code than the pinned code, or code outside the folder, would make the records name code that did not run.

## Applies to

- the command line, not a spec field
