# E-PTH-001

Severity: error

## Problem

A file path is absolute, starts with a drive, has a . or .. segment, contains a backslash or a colon, has a segment ending in a dot or space, passes through a symbolic link, or lies inside the path of another input.

## Fix

Write the path from the study root with no `.` or `..` segment, no `:` or `\`, and no segment ending in a dot or space; write `data/x.txt`, not `./data/x.txt`. List a folder or the files in it, not both.

## Why

The checker reads only files inside the study root. It never follows links, so a spec cannot point it at other files.

## Applies to

- `inputs[*].path` (named:input-path-safe)
- `inputs[*].path` (named:input-path-nested)
- `scope.end_of_run.follow_on[*].spec` (named:input-path-safe)
