# E-CNF-004

Severity: error

## Problem

The confirmation clusters are empty or overlap the selection clusters.

## Fix

List confirmation clusters that do not appear in the selection clusters.

## Why

A winner picked on some data looks better on that same data. Only fresh clusters give an honest test.

## Sources

- <https://doi.org/10.1038/s41562-016-0021>

## Applies to

- `design.confirmation.clusters` (named:selection-confirmation-disjoint), when mode = confirmatory
- `design.confirmation.clusters[*]` (named:selection-confirmation-disjoint), when mode = confirmatory
