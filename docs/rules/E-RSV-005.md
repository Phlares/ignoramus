# E-RSV-005

Severity: error

## Problem

design.resize declares a re-size that no test can use.

## Fix

In an exploratory study, remove [design.resize] or make the study confirmatory. In a study that re-sizes, give each claim a mean statistic.

## Why

The re-size sizes a confirmatory test from one observation per cluster. An exploratory study tests no claim, and the bounds of a quantile claim each read their own observation, so the count would size a test that does not run.

## Sources

- <https://www.fda.gov/media/78495/download>

## Applies to

- `design.resize` (named:resize-tested)
