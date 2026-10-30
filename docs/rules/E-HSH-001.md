# E-HSH-001

Severity: error

## Problem

An input is in neither place: the pin names no object in the box's store, and the file is missing or its SHA-256 differs from the pin.

## Fix

Fix the path, or update the sha256 if the file was changed on purpose. To pin a stored object, run `ignorarium import` and use the digest it prints.

## Why

The pin says which bytes the plan was approved on. A different file is a different study.

## Applies to

- `inputs[*].sha256` (named:input-hash-matches)
