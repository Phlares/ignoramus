# E-STP-002

Severity: error

## Problem

The file the reply stamps is not found, or its SHA-256 is not the reply's digest.

## Fix

Keep the file `stamp --request` wrote in the study's `stamps` folder, or name the stamped file with `--file`. Pass the reply for that file.

## Why

A reply covers the bytes whose digest it holds. The record names those bytes, so the box keeps a copy of them and records no reply it cannot match to a file.

## Sources

- <https://www.rfc-editor.org/rfc/rfc3161>

## Applies to

- the command line, not a spec field
