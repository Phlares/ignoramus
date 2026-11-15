# 0031 The card's three verdict words

Date: 2026-11-15. The words are built (the study card). This record dates them and adds the decision on a position label.

The box writes one `verdict` record per hypothesis: `pass`, `fail` or `none` (0013). A reader who sees `fail` can take it to mean "the claim is false". The test never shows that.

Decision:

- **Three words.** The card prints `pass` as "shown", `fail` as "not shown" and `none` as "no claim". One function holds the map (`card::sections::display_word`).
- **One fixed sentence.** When any verdict reads "not shown", the card adds: "Not shown" means the registered test did not reach its pass e-value. It does not show the claim is false.
- **No position label.** The card prints no second label that says where the estimate or the bound sits against the margin. Per stratum it prints the estimate, the bound and the margin. A reader takes the position from those numbers.
- **Text quotes the card.** A text written by hand about a result uses the card's word and the card's numbers.

Evidence for "no label". A three-way label was applied by rule to finished results: A, the estimate is beyond the margin; B, the estimate is inside and the bound is beyond; C, the bound is inside.

- It separates the two strata of the first confirmatory study: one reads B, the other A. Both read "not shown".
- C is "shown" under another name.
- A truth exactly on the margin (the litmus `edge` arm) reads A in 51 of 100 seeds and B in 49. The sign of the sample mean decides.
- B reads as "nearly passed". In the stratum that read B, the e-value reached 25.5 of the 200 it needed, and the chance of a pass at its true cost was near 0.
- A two-way label ("estimate beyond the margin", or nothing) is label A. It has the same fault on the `edge` arm.

Why:

- "Not shown" says what the test did. It makes no statement about the claim.
- The fixed sentence stops the most likely wrong reading of a null result.
- A label derived from the estimate is a second inference that no plan registered.

Case against:

- One word covers two states: an estimate inside the margin with a bound beyond it, and an estimate beyond the margin. The reader must look at the numbers.
- A common reporting practice sorts results into three classes by where the interval lies. It uses a two-sided interval. The card's bound is one-sided, so a label built on it cannot say "beyond the margin" with an error rate.
- A label that is valid at the stopping time needs its own registered analysis (#98).

What failure this prevents: a hand-written summary that reports a "not shown" as a failed claim, or as a near pass.

Sources:

- A registered-report guideline: classify a result by whether its interval lies below, spans or lies above the smallest effect of interest; do not report "spans" as a null. https://www.ebi.ac.uk/europepmc/webservices/rest/PMC8647608/fullTextXML
- Lakens and others, "Justify your alpha": describe a result by its effect size and relevance. https://eprints.whiterose.ac.uk/id/eprint/128599/8/Justify%20Your%20Alpha.pdf
