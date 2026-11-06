# E-CNF-002

Severity: error

## Problem

The confirmation sample is too small.

## Fix

Add confirmation inputs to each stratum the detail names, until each has the clusters the power gate needs. The detail states the simulated power and about how many clusters per stratum reach design.power. Raising design.units_per_cluster helps only as far as the icc allows, since the test takes one value per cluster. A mean range wider than the cluster means need also costs power. Two inputs that hold the same data are one cluster. In a study that re-sizes, add reserve inputs to each stratum the claim lists instead, and keep the first batch as declared.

## Why

A test with too few clusters misses real effects and exaggerates the ones it finds. The gate simulates the test the box runs: one value per cluster, a bounded betting e-value for each bound in each stratum the claim lists (all confirmation inputs when it lists none), and a pass only when every bound in every stratum reaches 1 / (the level the stream gives the study x alpha_share). It draws 4,000 replicates of each stratum's own clusters from a normal model with the sd and icc from the study in estimates, centred on power_at; for a quantile claim, the units' quantile lies at power_at. For a mean claim each simulated cluster mean is clipped to the declared range, as the test clips each unit; the two agree while the units stay inside the range, and the claim is about the clipped mean. The borrowed sd and icc are those of the source study's unit values clipped to the declared range, as the test clips them, and of several arms a mean claim takes the one whose cluster mean has the largest sd over the units its clusters give, with its own sd and icc together; a quantile claim takes the largest sd and the largest icc. Each simulation draws at most 2,000 million values, so a quantile claim on very large clusters fails closed instead of running for hours. The power is that of the joint event, so each of several strata needs more than the target alone. The seed comes from the spec hash and the hypothesis id, so every check of a spec gives the same power. A one-sided z-test needs fewer clusters than the e-value test, so a mean claim with fewer clusters than the z count fails first, without the simulation. The z count assumes the box estimates the statistic as the unweighted mean of the cluster means, and so uses the harmonic mean of the units the clusters give. The model is normal: a skewed outcome can have another power. A study that re-sizes is simulated whole: each replicate draws the first batch, sizes it as the re-size would from its spread about 0, fails when the reserve holds too few clusters, and tests the first batch then the clusters added, with no z count, since that assumes a fixed number of clusters.

## Sources

- <https://doi.org/10.1038/nrn3475>
- <https://doi.org/10.1136/bmj.e5661>
- <https://doi.org/10.1186/1471-2288-11-94>
- <https://arxiv.org/abs/2010.09686>
- <https://doi.org/10.1080/00401706.1982.10487790>

## Applies to

- `question.hypotheses[*]` (named:power-met), when mode = confirmatory
