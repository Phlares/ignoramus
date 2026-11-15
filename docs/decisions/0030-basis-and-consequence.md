# 0030 A basis for declared values, and the gate prints their consequence

Date: 2026-11-15. Decided. Nothing in this record is built yet.

A spec declares its design values as bare numbers: the margin of a claim, `power_at`, `design.power`, the range of a mean. The box reads each number and nothing about its origin. The gate prints one power and one size from them. Two mechanisms now serve every such value.

Decision:

- **Basis.** A declared design value can carry a basis: a kind, a source and a date. The kind `none` is allowed. It means a stated judgment with no source.
- **`none` is flagged, never refused.** In a confirmatory spec, `check` warns when a basis is absent or `none`. The card prints each basis and the count of bases marked `none`.
- **Consequence.** The gate prints what its own simulation gives for a declared value, over a small grid of that value or at a borrowed estimate. A consequence informs the person who approves the registration. It sizes nothing and blocks nothing.
- **The prints.**
  - Margin: when the study names a negative control with a known cost, the margin as a share of that cost.
  - `power_at`: the chance of a pass at the lending study's own estimate, with an interval (#76).
  - `design.power`: the joint power and the power per stratum.
  - Range: the clusters needed at two or three other ceilings, and the share of a named sample that each ceiling would clip. A written ceiling rule says whether it measures distance above the margin or on either side of it.
  - Selection: the share of resamples of the selection units in which the pick wins (#75).
  - Borrowed spread (0014): per estimate, the clusters behind it, an interval, the size over that interval, and the candidate whose spread the gate borrowed.
- **One meaning of `design.power`.** It is the joint target: the chance that every bound in every stratum passes (0013). The re-size asks each of k strata for `design.power`^(1/k) (0020).
- **No offset rule.** The box gives no default for `power_at` and no rule that derives it from the margin. Each `power_at` states its own basis.
- **No preset supplies a value.** A template or a profile may require a basis or a print. It supplies a number only where a named standard states that number.
- **Same shape, own issues.** Three more declared things follow the same two mechanisms: a group above the cluster (#111), the stated population as named attributes with a count per value, and input screens as named rules that `check` applies again.

Evidence. The first confirmatory study and the three planned after it. Each number was computed again from the records before it went into this record.

- Basis lines written by hand for the four studies: 5 of 5 margins, 3 of 5 `power_at` values and 2 of 4 power targets read `none`. Three kinds per value covered every case.
- The three `power_at` values marked `none` came from one house rule, "2 points below the margin". No file gives that rule a source.
- The gate printed power 0.98 at `power_at`. At the lending study's estimate the chance of a pass was 0.00, for the pick and for each of the four other candidates. For a planted pass (the litmus `good` arm) it was 1.00, and 0.985 to 1.00 over the interval.
- Two specs held `power = 0.95` where the reason on file was 0.95 per stratum over four strata. The box reads 0.95 as the joint target, which is 0.987 per stratum.
- The first study's ceiling sat 0.654 above the margin, and one stratum needed 276 clusters. Ceilings 0.400 and 0.300 above the margin need 174 and 135. Of 2,560 units, none clips at any of the three.
- On 8 selection units, the pick won 57% of 10,000 resamples.
- The lending study had 5 clusters per stratum. In one stratum the 95% interval of its icc ran from 0 to 0.66. Over the interval of the spread, the size ran from about 270 to about 420 clusters. The gate had borrowed the spread of the candidate with the largest sd, not that of the pick.

Why:

- A value's origin does not change the test's validity. It changes what a pass means. A reader cannot judge a margin without it.
- The box will run many kinds of study. Two mechanisms cover every declared value. One field and one gate line per value would not.
- A flag and not a refusal: a refused `none` invites an invented basis.
- Guidance on no-worse-than trials asks for the same items: a margin justified in the protocol, a stated assumed true difference, and a size reported over a range of inputs. A pilot gives the spread, not the target effect (0013).

Case against:

- A basis field invites filler. `check` already accepts an empty population (#68).
- The warning fires on every spec of the first family. People may stop reading it.
- A pass chance or a size table printed before registration can pull the author toward an easier `power_at`, margin or range.
- A lending study with 5 clusters gives a wide interval. The print can mislead in either direction.
- The sources are guidance for drug trials with fixed-size tests. The box bets. The mapping is an analogy.
- These are more spec fields and more gate lines, added before the pass that removes mechanisms.

What failure this prevents: a confirmatory study registered with a printed power of 0.98. Its lending study's estimates gave it no chance of a pass. It ran 5,319 rows and ended "not shown". No record said that the assumed truth had no source.

Open:

- The list of kinds for each value.
- A second selection rule (best worst stratum) waits for a simulation at borrowed estimates (#75).
- Issues for the population attributes and for the screens.
- This record sets no value.

Sources:

- ICH E9, "Statistical principles for clinical trials": the margin is set and justified in the protocol; state the assumed true difference. https://www.ema.europa.eu/en/documents/scientific-guideline/ich-e-9-statistical-principles-clinical-trials-step-5_en.pdf
- EMA, draft guideline on non-inferiority and equivalence comparisons in clinical trials: the margin comes first and the size second; a margin is never chosen because it is achievable. https://www.ema.europa.eu/en/documents/scientific-guideline/draft-guideline-non-inferiority-equivalence-comparisons-clinical-trials_en.pdf
- Guidance on pilot studies and sample size (secondary source): size under a range of plausible inputs; take the spread from a pilot, not the target effect. https://www.ebi.ac.uk/europepmc/webservices/rest/PMC5779280/fullTextXML
- Lakens and others, "Justify your alpha": state the reason for a design value before the data. https://eprints.whiterose.ac.uk/id/eprint/128599/8/Justify%20Your%20Alpha.pdf
- Power when all of G no-worse-than tests must pass: 1 − β/G per test. https://ar5iv.labs.arxiv.org/html/2402.11609
- An icc from one pilot of a cluster trial is too imprecise to size the main trial alone. https://www.ebi.ac.uk/europepmc/webservices/rest/PMC10507981/fullTextXML
