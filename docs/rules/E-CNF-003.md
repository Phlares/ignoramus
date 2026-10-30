# E-CNF-003

Severity: error

## Problem

A confirmatory hypothesis takes its sd and icc from no matching estimate of a finished study.

## Fix

Set estimates = { from = "<study id>" } to a study in this box that measured this hypothesis's contrast: the same outcome, measure and paired rule, and the same conditions, weights and params. A contrast on selected needs an estimate for each candidate. The study needs an estimate in each stratum the hypothesis lists, or a pooled one, with an icc, which takes 2 or more clusters.

## Why

A variance typed by hand can be too small, which makes the power gate pass a study that is too small. The box reads the sd and icc from the records of a finished study instead, so no one types them.

## Sources

- <https://doi.org/10.1038/nrn3475>
- <https://doi.org/10.1136/bmj.e5661>

## Applies to

- `question.hypotheses[*].estimates` (named:estimates-from-record), when mode = confirmatory
