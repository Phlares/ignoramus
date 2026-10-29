# 0009 Statistics engine

The `evidence` crate computes the statistics that decide whether an experiment found something. It does no I/O and has no dependencies.

Terms used below:

- A tree is a root hypothesis plus declared follow-up hypotheses (its children, and their children).
- A trigger is an e-process on spawn data. It decides whether a hypothesis's children run.
- A final test is an e-process on separate report data. It tests each hypothesis that ran.
- An e-value is a non-negative number whose expected value is at most 1 under the null. A large e-value is evidence against the null.
- A stream slot is the level a tree draws from the e-LOND stream (defined below). The slot closes with the tree's e-value.

Decision:

- The crate computes e-processes, an online false-discovery stream (e-LOND), a tree split for spawned follow-up tests, and scan p-values. It reads no files, network, clock or environment. Every random draw comes from an `Rng` that the caller seeds (xoshiro256**; Blackman and Vigna 2021). The same seed gives the same result.
- **E-process.** Each observation x maps to a score s in [-1, 1]. Under the null, the mean of s given the past is at most 0. Wealth is the product of `1 + λ_i s_i`. The bet is `λ_i = clamp(S / (1 + Q), 0, 1/2)`. S is the sum of past scores. Q is the sum of past squared scores. So the bet is the running mean score over the running mean squared score, with one pseudo-observation, capped at 1/2 (the aGRAPA bet; Waudby-Smith and Ramdas). The bet is fixed before x_i is seen. The 1/2 cap keeps every factor at or above 1/2, so wealth never reaches 0 (Ramdas, Grünwald, Vovk and Shafer: never bet the farm). Wealth is kept in log space, so long runs neither overflow to NaN nor underflow to 0.
  - `Sign`: s = sign(x). Null: P(x > 0) ≤ P(x < 0). This is the sign test. It assumes no shape.
  - `Clip { scale }`: s = clamp(x / scale, -1, 1). Null: x is symmetric about 0 given the past. The study sets the scale, normally to its smallest effect of interest.
  - `Bounded { lo, hi, mean }`: s = (x - mean) / max(hi - mean, mean - lo). Null: E[x | past] ≤ mean, with x in [lo, hi].
  - `Tail::Upper` bets on s. `Tail::Lower` bets on -s. `Tail::Both` is the mean of the two wealths. A mean of e-values is an e-value under any dependence (Vovk and Wang). `Both` needs the null with equality: symmetry for `Clip`, E[x | past] = mean for `Bounded`.
  - A stop threshold freezes the process the first time its value reaches the threshold. A stopped e-process still gives an e-value. Triggers stop at their threshold θ. Sequential tests stop at 1/α. Fixed-size tests never stop.
- **e-LOND stream** (online false discovery rate control with e-values; Xu and Ramdas). Slot t gets level `α γ_t (D + 1)`. D is the number of discoveries closed before slot t opened. Slot t makes a discovery when its level is above 0 and its e-value is at least 1/level.
  - A level counts only discoveries closed before the slot opened. The proof needs one fact. A discovered slot's level is at most `α γ_t` times the final discovery count. That holds whatever order slots close in. So trees may run at the same time.
  - A slot that opens and never closes stays used. Later levels do not count it.
  - `Bounded { m }`: γ_t = 1/m for t ≤ m, and 0 after. Slot m + 1 and later never make a discovery, even at e = +inf (Robertson, Wason and Ramdas).
  - `Unbounded`: γ_t = C ln(max(t, 2)) / (t e^√(ln t)), with C = 0.07720838 (Javanmard and Montanari 2018, eq. 31). The series sums to about 0.976.
- **Tree split with return** (Choodari-Oskooei et al.). A tree declares a weight w_k for every hypothesis it may test: the root and each declared child, spawned or not. In a stream, the tree's α is its slot's level. A hypothesis that ran gets level `α w_k / W`. W is the sum of the weights of the hypotheses that ran. A hypothesis that did not run gets level 0. Its share returns to the ones that ran. Hypothesis k is rejected when its e-value is at least 1/level_k.
  - The tree's e-value is the weighted mean `Σ w_k e_k / W` over the hypotheses that ran. It reaches 1/α whenever any hypothesis is rejected. The tree hands this value to the stream. A tree can therefore be a discovery with no single hypothesis rejected.
  - Validity condition: which hypotheses ran must not depend on the data their e-values come from. A trigger reads one set of seeds. The final test reads a disjoint set. Under this condition, strong family-wise error control holds by Markov's inequality and the union bound. It holds under any dependence between sibling hypotheses.
  - Without the condition, the return can raise the family-wise error. The share would return exactly when the data looked weak, so the remaining tests would run at a higher level on selected data.
- **Scan p-values** (look-elsewhere correction; Particle Data Group). A scan tests several conditions on the same units. Each unit has a paired difference d_i. Condition k has statistic T_k. There are B flip draws. Draw b gives statistic T*_bk.
  - The statistic is `z = Σ ε_i d_i / sqrt(Σ d_i²)`, with ε_i the flip sign (+1 for the observed data). It is 0 when Σ d_i² = 0. A flip does not change Σ d_i². So z orders the same way as the paired t statistic. The tail sets T: Upper uses z, Lower uses -z, Both uses |z|.
  - Local: p_k = (1 + #{b : T*_bk ≥ T_k}) / (B + 1). The observed statistic counts as one more draw, so the denominator is B + 1. The +1 keeps a p-value above 0 (Phipson and Smyth).
  - Global: p_k = (1 + #{b : max_j T*_bj ≥ T_k}) / (B + 1). This is single-step maxT (Westfall and Young).
  - Each flip draw gives one random sign per unit and applies it to every condition. Shared flips keep the dependence between conditions (Girardi et al.).
- **Null-tree suite.** The suite simulates trees and measures family-wise error (FWER), false discovery rate (FDR) and power. It follows the FDA guidance on adaptive designs. Type I error comes from simulation. A grid covers nuisance values. Each scenario has its own seed. Monte Carlo error is reported.
  - Model: unit i of hypothesis j in a data role is `mean_j + sqrt(ρ) Z_i + sqrt(1 - ρ) N_ij`. Z_i is shared by all hypotheses of the tree. N_ij is that hypothesis's own noise. Both are standard normal. The spawn role and the report role have separate draws. A hypothesis is null when its mean is 0. A tree has an effect when its root's mean is not 0.
  - Effect placement: in a tree with an effect, the effect sits on the root only (children are null), or on every hypothesis. The table says which. Effects are in units of the noise standard deviation.
  - Triggers use `Clip` with scale 1, `Upper`, stop at θ. Final tests use `Clip` with scale 1, `Both`, no stop.
  - Size: 100,000 trees for each single-tree scenario. 10,000 streams of 40 trees for each stream scenario. Fewer is fine when the Wilson upper end is below the target.
  - Monte Carlo error is the upper end of the 95% Wilson score interval of the mean (Wilson 1927; Brown, Cai and DasGupta 2001). The results are in [0, 1], so the interval is conservative.
  - A bound "holds" when that upper end is at most the target. The target is α. FWER is checked on every single-tree row. FDR is checked on single-tree rows with no effect, and on every stream row.
  - Planted defect: each firing hypothesis also tests 100 undeclared null children. Each is tested alone at the slot's full level. The scenario has 3 declared children, depth 1, 20 units, ρ = 0, θ = 2, α = 0.05, and an effect of 2 on the root. With 2,000 replicates the FWER is 0.080 (upper end 0.093), so the bound fails. With no undeclared children it is 0.000 (upper end 0.002), so the bound holds. The suite flags the defect.

Results of the full run (release build, about 28 s). Each row is one scenario. Columns: α; gamma sequence; trees per stream; declared children per hypothesis; depth; units per data role; ρ; θ; chance a tree has an effect; where the effect sits and its size; replicates; FWER mean / upper end; FDR mean / upper end; power. Power is the share of trees with an effect that were discovered. A dash means the scenario has no such trees. Single-tree rows use one slot at level α (`Bounded` with m = 1).

| α | gamma | trees | children | depth | units | ρ | θ | effect chance | effect | replicates | FWER mean / upper | FDR mean / upper | power |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0.05 | Bounded m=1 | 1 | 3 | 1 | 20 | 0.0 | 2 | 0.00 | none | 100,000 | 0.00066 / 0.00084 | 0.00066 / 0.00084 | - |
| 0.05 | Bounded m=1 | 1 | 3 | 1 | 20 | 0.0 | 2 | 1.00 | root 1.0 | 100,000 | 0.00003 / 0.00009 | 0.00000 / 0.00004 | 0.29329 |
| 0.05 | Bounded m=1 | 1 | 3 | 1 | 20 | 0.0 | 20 | 0.00 | none | 100,000 | 0.00084 / 0.00104 | 0.00084 / 0.00104 | - |
| 0.05 | Bounded m=1 | 1 | 3 | 1 | 20 | 0.0 | 20 | 1.00 | root 1.0 | 100,000 | 0.00002 / 0.00007 | 0.00000 / 0.00004 | 0.35056 |
| 0.05 | Bounded m=1 | 1 | 3 | 2 | 20 | 0.0 | 2 | 0.00 | none | 100,000 | 0.00056 / 0.00073 | 0.00056 / 0.00073 | - |
| 0.05 | Bounded m=1 | 1 | 3 | 2 | 20 | 0.0 | 2 | 1.00 | root 1.0 | 100,000 | 0.00002 / 0.00007 | 0.00000 / 0.00004 | 0.22339 |
| 0.05 | Bounded m=1 | 1 | 3 | 2 | 20 | 0.0 | 20 | 0.00 | none | 100,000 | 0.00093 / 0.00114 | 0.00093 / 0.00114 | - |
| 0.05 | Bounded m=1 | 1 | 3 | 2 | 20 | 0.0 | 20 | 1.00 | root 1.0 | 100,000 | 0.00003 / 0.00009 | 0.00000 / 0.00004 | 0.34486 |
| 0.05 | Bounded m=1 | 1 | 3 | 1 | 20 | 0.9 | 2 | 0.00 | none | 100,000 | 0.00048 / 0.00064 | 0.00054 / 0.00070 | - |
| 0.05 | Bounded m=1 | 1 | 3 | 1 | 20 | 0.9 | 2 | 1.00 | root 1.0 | 100,000 | 0.00001 / 0.00006 | 0.00000 / 0.00004 | 0.29051 |
| 0.05 | Bounded m=1 | 1 | 3 | 1 | 20 | 0.9 | 20 | 0.00 | none | 100,000 | 0.00084 / 0.00104 | 0.00085 / 0.00105 | - |
| 0.05 | Bounded m=1 | 1 | 3 | 1 | 20 | 0.9 | 20 | 1.00 | root 1.0 | 100,000 | 0.00001 / 0.00006 | 0.00000 / 0.00004 | 0.34647 |
| 0.05 | Bounded m=1 | 1 | 3 | 2 | 20 | 0.9 | 2 | 0.00 | none | 100,000 | 0.00063 / 0.00081 | 0.00078 / 0.00097 | - |
| 0.05 | Bounded m=1 | 1 | 3 | 2 | 20 | 0.9 | 2 | 1.00 | root 1.0 | 100,000 | 0.00001 / 0.00006 | 0.00000 / 0.00004 | 0.23820 |
| 0.05 | Bounded m=1 | 1 | 3 | 2 | 20 | 0.9 | 20 | 0.00 | none | 100,000 | 0.00080 / 0.00100 | 0.00080 / 0.00100 | - |
| 0.05 | Bounded m=1 | 1 | 3 | 2 | 20 | 0.9 | 20 | 1.00 | root 1.0 | 100,000 | 0.00005 / 0.00012 | 0.00000 / 0.00004 | 0.34809 |
| 0.5 | Bounded m=1 | 1 | 3 | 1 | 20 | 0.0 | 2 | 0.00 | none | 100,000 | 0.05533 / 0.05676 | 0.06048 / 0.06197 | - |
| 0.5 | Bounded m=1 | 1 | 3 | 2 | 20 | 0.0 | 2 | 0.00 | none | 100,000 | 0.05520 / 0.05663 | 0.06025 / 0.06174 | - |
| 0.5 | Bounded m=1 | 1 | 3 | 1 | 20 | 0.9 | 2 | 0.00 | none | 100,000 | 0.05378 / 0.05520 | 0.06240 / 0.06392 | - |
| 0.5 | Bounded m=1 | 1 | 3 | 2 | 20 | 0.9 | 2 | 0.00 | none | 100,000 | 0.05265 / 0.05405 | 0.06279 / 0.06431 | - |
| 0.5 | Bounded m=1 | 1 | 16 | 1 | 20 | 0.0 | 2 | 1.00 | root 2.0 | 100,000 | 0.00304 / 0.00340 | 0.00000 / 0.00004 | 0.99997 |
| 0.05 | Unbounded | 40 | 3 | 1 | 30 | 0.5 | 5 | 0.00 | none | 10,000 | 0.00000 / 0.00038 | 0.00000 / 0.00038 | - |
| 0.05 | Unbounded | 40 | 3 | 1 | 30 | 0.5 | 5 | 0.25 | all 1.5 | 10,000 | 0.00000 / 0.00038 | 0.00000 / 0.00038 | 0.91431 |
| 0.05 | Bounded m=40 | 40 | 3 | 1 | 30 | 0.5 | 5 | 0.00 | none | 10,000 | 0.00000 / 0.00038 | 0.00000 / 0.00038 | - |
| 0.05 | Bounded m=40 | 40 | 3 | 1 | 30 | 0.5 | 5 | 0.25 | all 1.5 | 10,000 | 0.00170 / 0.00272 | 0.00015 / 0.00064 | 0.99936 |

Every bound holds. The first 16 rows are the strict grid (α = 0.05). The next 5 rows are the loose grid (α = 0.5). The last 4 rows are the stream grid.

What the margins mean:

- At α = 0.05, a null e-value test with 20 units rejects about 1 time in 1,300. That is far below 0.05. It is the price of validity at any stopping time. So the strict grid cannot tell a dropped split from a kept one.
- The loose grid at α = 0.5 shows the split at work. A null hypothesis over 20 units rejects 6.2% of the time at that level. A null root fires a trigger at θ = 2 for 17.8% of trees. Both rates are from 1,000,000 seeded draws. So the four ordinary loose rows hold even with no split.
- The partly-null row is the last loose row: a firing root with 16 null children. With the split its FWER is 0.00304 (upper end 0.00340). Without the split each child is tested at the full level, so its FWER is about 0.64. That figure is computed as 1 - (1 - 0.062)^16. This row is the one that fails when the split is dropped.
- The planted defect needs 100 undeclared children to show. At 10 or 20 undeclared children the FWER is 0.008 and 0.013, still under 0.05. The true FWER crosses 0.05 near 65 to 70 children.
- FDR can exceed FWER in a null row. The tree's e-value is a mean. It can reach 1/α without any single hypothesis passing its own level.

Planning consequence:

- An e-value test needs more units than the classical z-formula gives for the same power. A 10-unit `Both` test at α = 0.05 cannot reach 1/α = 20. The first bet is 0 and later bets are at most 1/2. With ten scores of +1 the upper wealth is 1.5^9 = 38.4, the lower wealth stays 1, and their mean is 19.7.
- Sizing a confirmatory e-value test needs a simulated power check.

Why:

- E-values stay valid at any stopping time. A mean of e-values stays valid under any dependence. Both facts hold without a model of how the data were collected.
- The stream counts only closed discoveries. Trees may then run at the same time and close in any order, and the guarantee holds.
- An unrun hypothesis's share returns to those that ran. This keeps power, and it is valid under the stated condition.
- The Wilson interval is sound near 0. The error rates here sit near 0, where the usual normal interval is not.

Sources:

- Xu and Ramdas, "Online multiple testing with e-values", https://arxiv.org/abs/2311.06412
- Robertson, Wason and Ramdas, "Online multiple hypothesis testing", https://arxiv.org/abs/2208.11418
- Javanmard and Montanari (2018), "Online rules for control of false discovery rate and false discovery exceedance", Annals of Statistics 46(2), eq. 31
- Waudby-Smith and Ramdas, "Estimating means of bounded random variables by betting", https://arxiv.org/abs/2010.09686
- Ramdas, Grünwald, Vovk and Shafer, "Game-theoretic statistics and safe anytime-valid inference", https://arxiv.org/abs/2210.01948
- Vovk and Wang, "E-values: calibration, combination and applications", https://arxiv.org/abs/1912.06116
- Choodari-Oskooei et al., "Adding new experimental arms to randomised clinical trials: impact on error rates", https://arxiv.org/abs/1902.05336
- Girardi et al., "Post-selection inference in multiverse analysis (PIMA)", Psychometrika 89 (2024), https://arxiv.org/abs/2210.02794
- Westfall and Young (1993), "Resampling-Based Multiple Testing"
- Phipson and Smyth (2010), "Permutation p-values should never be zero", Stat. Appl. Genet. Mol. Biol. 9(1)
- Particle Data Group, Review of Particle Physics 2025, "Statistics", https://pdg.lbl.gov/2025/reviews/rpp2025-rev-statistics.pdf
- FDA (2019), "Adaptive Designs for Clinical Trials of Drugs and Biologics", https://www.fda.gov/media/78495/download
- Wilson (1927), JASA 22(158)
- Brown, Cai and DasGupta (2001), "Interval estimation for a binomial proportion", Statistical Science 16(2)
- Blackman and Vigna (2021), "Scrambled linear pseudorandom number generators", ACM TOMS 47(4), https://prng.di.unimi.it/
