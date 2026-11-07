# E-CNF-005

Severity: error

## Problem

Confirmation or reserve data was used by an earlier study.

## Fix

Choose one: if the earlier study is not registered yet, add this study to its family (every member is then locked, and each takes a stream slot); run this study as exploratory on the same data (no claim); or give it fresh data. A member of the earlier study's family needs nothing.

## Why

Reused data has already shaped a decision. A test on it is not independent. A reserve input is confirmation data held back, so it must be fresh too. Data that shaped a decision cannot also be its evidence. Studies locked together before any data is scored may share data: each keeps its own stream slot, and the stream controls the false discovery rate under any dependence.

## Sources

- <https://doi.org/10.1038/s41562-016-0021>
- <https://arxiv.org/abs/2311.06412>
- <https://arxiv.org/abs/2407.15733>

## Applies to

- `inputs[*].path` (named:data-fresh), when mode = confirmatory
