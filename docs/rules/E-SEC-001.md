# E-SEC-001

Severity: error

## Problem

A string looks like a secret or personal data, of kind

## Fix

Remove the value from the spec, or move it out of the spec and refer to it by name. Do not copy the value into any other text.

## Why

A registered plan is kept and shared. A secret or personal detail in it cannot be taken back.

## Sources

- <https://github.com/gitleaks/gitleaks>

## Applies to

- `**` (named:secret-scan)
