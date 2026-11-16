# E-RPL-001

Severity: error

## Problem

study.replicates.done is not the hash of the original's done record.

## Fix

Copy the hash from the Study ended line of the original's card into study.replicates.done. If the original ended in another way than you planned for, read its card again before you plan the replication.

## Why

The pin names the end of the original that the replication repeats. The box finds the original by its id and checks the pin against its signed log, so a plan cannot name an end the original does not have.

## Applies to

- `study.replicates` (named:replicates-done)
