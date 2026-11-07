# E-CNF-004

Severity: error

## Problem

The plan has no confirmation input, or a confirmation or reserve input holds the same data as a selection or calibration input, or a reserve input holds the data of a confirmation input or of an earlier reserve input.

## Fix

Add a confirmation input. Give each confirmation and reserve input data that no selection or calibration input holds, and each reserve input data that no confirmation input and no earlier reserve input holds: not the same bytes, and not the same text in other case or spacing.

## Why

A winner picked on some data looks better on that same data. Only fresh data gives an honest test. The content decides, not the file name. A model calibrated on some data has adapted to it; its error there is not its error on new data. A reserve cluster the re-size adds must be new to the test too, or the test counts the same data twice. The rule binds a confirmatory plan, and a plan with triggers in any mode: a trigger decides on the selection data which children run, and the children are tested on the confirmation data, so the tests stay valid only when the two are apart. An exploratory plan without triggers may share data.

## Sources

- <https://doi.org/10.1038/s41562-016-0021>
- <https://arxiv.org/html/2311.09755v2>
- <https://www.fda.gov/media/78495/download>

## Applies to

- `inputs` (named:confirmation-disjoint)
- `inputs[*].sha256` (named:confirmation-disjoint)
