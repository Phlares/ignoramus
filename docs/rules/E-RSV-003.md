# E-RSV-003

Severity: error

## Problem

A reserve input has no SHA-256 pin.

## Fix

Pin each reserve input: set sha256 to the 64 lowercase hex characters of its file's SHA-256, or import the file and use the digest `ignorarium import` prints.

## Why

The re-size takes reserve clusters in the order of their pins, so the pins fix which data the test adds before any result is seen. Without a pin the order, and so the added data, could be picked after the first results. A test is honest on data fixed in advance.

## Sources

- <https://ar5iv.labs.arxiv.org/html/1902.10811>

## Applies to

- `inputs[*].sha256` (named:reserve-pinned), when mode = confirmatory
