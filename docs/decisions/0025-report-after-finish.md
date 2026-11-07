# 0025 Report after the study finishes

`ignorarium report <id> [--json]` prints the verdicts of a finished study. It refuses every other study. It verifies the record log, appends one signed `opened` record, and only then prints.

Decision:

- **Blinding ends at finish.** `report` refuses every study that is not finished, blind or not, and points to `status`. A finished study reports in full, blind or not. There is no separate unblind step.
- **Finished.** A study is finished when its log holds all of these:
  - the analysis record;
  - every estimate and verdict record the analysis promises;
  - for a study that holds a stream slot, the close of that slot.

  The finish time is the later of the last counted estimate or verdict record and the close record. A repeat of an estimate or verdict is not counted. With neither, it is the time of the analysis record. A study whose results are complete reports even if a stop record follows them. A stopped study that never finished refuses and gives its stop reason.
- **Every read is recorded.** `report` verifies the log (hashes, signatures, chain) under the log's lock, with the code `verify` uses. A broken log, in the study or in its stream, is refused, and the message names the record or line. The report then appends one signed `opened` record with body `{}`, and prints only after that append. A refused report appends nothing. The log holds each read, and the chain fixes the last record the read saw.
- **Contents.** Per hypothesis: the verdict, its basis, the e-value and level of a test, and the reason when the log holds one. Per mean claim and group: the units below `lo`, above `hi`, and in all. The count is descriptive and never changes a verdict. The text report prints each number in full, as JSON does. A quantile claim, or an analysis record written before the counts existed, prints no count.
- **Family.** A study in a family names the family. It lists every other member, the root first, then in lock order.
  - An unfinished member shows as `not finished`. A member with no log shows as `not registered`.
  - A finished member shows its finish time and the time of its first `opened` record, or `never`. It is marked `before` or `after` this study's finish time.
  - The JSON `before` is true when that member finished earlier. A reader sees which results were visible while this study ran.
  - A fault in the log of another member refuses the report: `cannot report {id}: study {other}: {fault}`. The log of this study stays unchanged. A fault in the box's register records or leaves also refuses the report.

Why:

- Regulators ask that the analysis plan be final before anyone breaks the blind (ICH E9). Finish is the point where the plan has run to its end, so the box shows results from there.
- Knowing interim results can steer later choices (FDA 2019). The box limits access to them until the study is done, and `status` shows progress without effect sizes.
- A log of each read shows when the results were opened (FDA 2019; ICH E9).
- In physics blind analysis, the result is accepted after unblinding without changing any step (Junk and Lyons). An analysis blind in psychology shows the outcome only after the plan is settled (Flexible yet fair).
- Endpoints and analyses are fixed in advance and not changed after unmasking (FDA 2022).
- In a family of studies, each study reports alone, cites earlier members, and lists the pending ones (PCI RR). Results from different stages are hard to read when interim results may have leaked. EMA 2007 asks for the access log with them.
- A report reuses the verification that `verify` runs. One code path decides what a sound log is.

Case against:

- The finish time says when results became visible through the box. It does not say when anyone looked.
- The record log is plain JSON. The `opened` record logs reads through the box only. A person who reads the file directly leaves no trace. The blind rests on good faith, as in physics blind analysis (Junk and Lyons).
- In a family on shared data, an early member's means hint at later members' effects (Żebrowska et al.). No member can change after the first one registers. e-LOND is the stream's rule for giving each study its level and counting discoveries. It controls the false discovery rate under any dependence (Xu and Ramdas). That includes shared data. Credibility rests on the planned family line.
- The `opened` record is signed before the print. A failed print still leaves the record. The record marks the box releasing the results, not a reader seeing them.

Sources:

- ICH E9 (1998), "Statistical Principles for Clinical Trials", https://www.govinfo.gov/content/pkg/FR-1998-09-16/html/98-24754.htm
- "Flexible yet fair: blinding analyses in experimental psychology" (Bayesian Spectacles), https://www.bayesianspectacles.org/flexible-yet-fair-blinding-analyses-in-experimental-psychology/
- Junk and Lyons, "Reproducibility and Replication of Experimental Particle Physics Results", https://arxiv.org/abs/2009.06864
- FDA (2022), "Multiple Endpoints in Clinical Trials", https://www.fda.gov/media/162416/download
- FDA (2019), "Adaptive Designs for Clinical Trials of Drugs and Biologics", https://www.fda.gov/media/78495/download
- EMA (2007), "Reflection paper on methodological issues in confirmatory clinical trials planned with an adaptive design", https://www.ema.europa.eu/en/documents/scientific-guideline/reflection-paper-methodological-issues-confirmatory-clinical-trials-planned-adaptive-design_en.pdf
- PCI RR, "Guide for authors", https://rr.peercommunityin.org/help/guide_for_authors
- Xu and Ramdas, "Online multiple testing with e-values", https://arxiv.org/abs/2311.06412
- Żebrowska, Posch and Magirr, "Maximum type I error rate inflation from sample size reassessment when investigators are blind to treatment labels", Statistics in Medicine (2016), https://www.ebi.ac.uk/europepmc/webservices/rest/PMC4851240/fullTextXML
