# E-STP-003

Severity: error

## Problem

The stamped file lacks this study's register line, or holds a box line with another value.

## Fix

Stamp the file `stamp --request` writes, unchanged. A file of your own must hold the register line exactly as the box writes it, with LF line ends.

## Why

A stamp of a study must cover its registration. The record says which box lines the file holds, and each of those must be the value the box computes from the log, so the record never vouches for a value the box did not compute.

## Applies to

- the command line, not a spec field
