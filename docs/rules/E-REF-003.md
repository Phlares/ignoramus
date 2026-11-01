# E-REF-003

Severity: error

## Problem

The plan takes estimates from a study that ran other code, and declares no deviation from it.

## Fix

Pin the source study's payload, environment and model and calibration inputs, or add a `[[pins.deviations]]` entry with `from`, `reason` and `timing`. A deviation names a study that a confirmatory hypothesis with a claim takes estimates from, and gives a reason.

## Why

Estimates describe the code that produced them. A pipeline is frozen before the confirmatory data exist, and a change is reported with its reason and when it happened.

## Sources

- <https://journals.plos.org/plosbiology/article?id=10.1371/journal.pbio.3000246>
- <https://journals.plos.org/ploscompbiol/article?id=10.1371/journal.pcbi.1010571>
- <https://doi.org/10.1177/25152459231213802>

## Applies to

- `question.hypotheses[*].estimates` (named:estimates-same-code)
- `pins.deviations[*].from` (named:deviation-borrowed)
- `pins.deviations[*].reason` (non-empty)
