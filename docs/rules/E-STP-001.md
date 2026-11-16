# E-STP-001

Severity: error

## Problem

The reply is not a granted RFC 3161 time-stamp reply over a SHA-256 digest.

## Fix

Pass the `.tsr` file the authority sent back, not the stamped file or an error page. When the authority refused, ask it again, or ask another, with `-sha256`.

## Why

Only a granted reply holds a time stamp. The box checks the digest against the file with SHA-256, so it records no reply over another digest.

## Sources

- <https://www.rfc-editor.org/rfc/rfc3161>

## Applies to

- the command line, not a spec field
