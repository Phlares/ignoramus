# 0022 Bet cap three quarters

The betting e-process (0009) caps each bet at `MAX_BET`. The cap was 1/2. It is now 3/4.

Decision:

- **One constant.** `MAX_BET` = 0.75, one value for the whole box. Every factor `1 + λ s` stays at or above 1/4, so wealth never reaches 0. It is the value we start with, informed by the sources below, to be tuned after the first study.
- **Below 1.** A cap of 1 lets one score at the far end give ln 0 = −inf. Once both sides of a `Both` test reach −inf, their mean is NaN. A test pins the cap below 1.
- **Fail closed.** Every e-value test decides through `evidence::rejects(e, level)`: e reaches 1 / level. A NaN never does, nor anything at a level of 0. The power simulation used `e < 1 / level` for a fail, which is false for NaN, so it counted a NaN as a pass. The analysis kept a NaN e-value as the largest float. It now keeps it as 0, so the claim fails.

Mechanism. The bet is the aGRAPA bet, `S / (1 + Q)`, capped (0009). With units tight against a wide declared range, each score is small and of one sign, so the bet reaches the cap within a few observations and stays there. Past that point the units needed scale about as 1 / cap. For a "mean below ln 1.04" claim on [−0.614706, 0.693147] at level 0.005 and power 0.894, with unit sd 0.0133 to 0.0217, a test needs about 400 units at a cap of 1/2, 281 at 3/4 and 225 at 0.99. Any test that assumes only the range needs at least about 183. Each count is simulated with 4,000 replicates.

Why 3/4 and not higher:

- One observation at the ceiling multiplies wealth by 1 − cap: ln 0.25 = −1.39 nats at 3/4, and −4.6 nats at 0.99, against the ln 200 = 5.3 nats a test at level 0.005 needs.
- With 0.5% of units at the ceiling, a cap of 0.99 never reaches the power, while 3/4 needs about 600 units. With 1% at the ceiling, 3/4 needs more than 1/2: about 1,630 units against 1,190.
- At narrow ranges the best bet lies inside the range, so a high cap over-bets.

Validity:

- Any bet in [0, 1) fixed before its observation keeps wealth a test supermartingale under the null, so by Ville's inequality the chance it ever reaches 1 / level is at most the level. The cap changes power, not validity.
- The evidence is the re-run of the 0009 tree suite and of the 0020 re-size suite, and the two-point nulls added to 0020. Their cluster observations take two values with mean at the margin, at the ends of the range or at its top and just below the margin, so the scores reach the top of the range, where the cap acts. Every bound holds. The largest null rejection share in the re-size suite is 0.00300 at level 0.05, and the largest share that crossed 1 / level at any observation is 0.01468, both from the skew 0.03 null. 0009 and 0020 give the tables.
- The NaN path fails closed, as above.

Case against:

- At 3/4 a test still needs about 1.5 times the lower bound.
- When 1% or more of units sit at the ceiling, 3/4 needs more units than 1/2.
- At narrow ranges, near the margin, a higher cap can need slightly more units.
- The re-size stops nearly every replicate of the ends and skew 0.3 nulls, so in the staged design only the skew 0.03 null runs long enough to test the cap.
- A higher cap gives the null e-values a heavier right tail. The claim test's null check now tests the mean of the last factor, since the sample mean of 20,000 e-values fell more than 4 standard errors below 1 in about 1 seed in 5.

Open:

- Tune the cap after the first study.
- A mixture over caps is the alternative if ceiling outliers appear. In every simulated case it was never the worst, and it needed about 10% more units than the best single cap.

Sources:

- Waudby-Smith and Ramdas, "Estimating means of bounded random variables by betting": hedged betting confidence intervals use c = 1/2 or 3/4 as defaults, and any c in [0, 1) is valid. https://ar5iv.labs.arxiv.org/html/2010.09686
- Clerico, "On the optimality of coin-betting for mean estimation": every coin bet up to the range limit is valid and maximal. https://arxiv.org/html/2412.02640v2
- Agrawal, Ram and Ramdas, "On stopping times of power-one sequential tests: tight lower and upper bounds": the expected sample size of any level-α power-one test is at least log(1/α) / KL_inf. https://arxiv.org/html/2504.19952
- Ramdas, Grünwald, Vovk and Shafer, "Game-theoretic statistics and safe anytime-valid inference": never bet the farm; bet a fraction. https://ar5iv.labs.arxiv.org/html/2210.01948
- Sokolova and Sokolov, "E-values for adaptive clinical trials: anytime-valid monitoring in practice": a fixed bet fraction must be tuned to the design alternative, and there under-betting cost more than over-betting. https://arxiv.org/html/2602.06379v1
- Ramdas and Wang, "Hypothesis testing with e-values": products of near all-in bets have the largest variance under the null, and can go to zero. https://arxiv.org/html/2410.23614
