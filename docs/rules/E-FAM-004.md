# E-FAM-004

Severity: error

## Problem

A family member's confirmation or reserve data is selection data of this study or of another member.

## Fix

Give the member confirmation and reserve data that no selection input in the family holds: not the same bytes, and not the same text in other case or spacing. Or remove the member from the family and run it as exploratory on this data (no claim).

## Why

A condition picked on some data looks better on that same data. A member may test a condition that a selection stage in the family picked, so its evidence must come from other data. The content decides, not the file name.

## Sources

- <https://www.fda.gov/media/78495/download>

## Applies to

- `family.members[*]` (named:family-selection), when mode = confirmatory
