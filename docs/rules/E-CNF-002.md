# E-CNF-002

Severity: error

## Problem

The confirmation sample is too small.

## Fix

Add confirmation inputs to each stratum the detail names, until each has the clusters the power gate needs. The detail states the simulated power and about how many clusters per stratum reach design.power. Raising design.units_per_cluster helps only as far as the icc allows, since the test takes one value per cluster. A mean range wider than the cluster means need also costs power. Two inputs that hold the same data are one cluster.

## Why

A test with too few clusters misses real effects and exaggerates the ones it finds. The gate simulates the test the box runs: one value per cluster, a bounded betting e-value for each bound in each stratum the claim lists (all confirmation inputs when it lists none), and a pass only when every bound in every stratum reaches 1 / (design.alpha x alpha_share). It draws 4,000 replicates of each stratum's own clusters from a normal model with the sd and icc from the study in estimates, centred on power_at; for a quantile claim, the units' quantile lies at power_at. The power is that of the joint event, so each of several strata needs more than the target alone. The seed comes from the spec hash and the hypothesis id, so every check of a spec gives the same power. A one-sided z-test needs fewer clusters than the e-value test, so a mean claim with fewer clusters than the z count fails first, without the simulation. The z count assumes the box estimates the statistic as the unweighted mean of the cluster means, and so uses the harmonic mean of the units the clusters give. The model is normal: a skewed outcome can have another power.

## Sources

- <https://doi.org/10.1038/nrn3475>
- <https://doi.org/10.1136/bmj.e5661>
- <https://doi.org/10.1186/1471-2288-11-94>
- <https://arxiv.org/abs/2010.09686>
- <https://doi.org/10.1080/00401706.1982.10487790>

## Applies to

- `question.hypotheses[*]` (named:power-met), when mode = confirmatory
