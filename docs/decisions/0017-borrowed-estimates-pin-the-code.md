# 0017 Borrowed estimates pin the code

Date: 2026-11-01.

A confirmatory hypothesis can take its sd and icc from a finished study (0014). Those numbers describe the code that measured them. This record fixes how a finished study names its code, what counts as the same code, and how a plan that runs other code says so.

Decision:

- **The instrument.** The `analysis` record of a finished run names its `instrument`, the code its estimates describe:
  - `payload`: the payload folder's tree hash, as the register record holds it (0012);
  - `environment`: the environment every row record names, `host` or the image's store digest; `""` when the rows name more than one (a study resumed in a rebuilt environment) or none;
  - `inputs`: each model and calibration input present, as (path, role, store digest), sorted by path.

  Selection and confirmation inputs are data, not code, so they stay out. An `analysis` record written before the field existed reads as an empty instrument, and its study stays finished.
- **The same code.** For each finished study that a confirmatory hypothesis with a claim takes estimates from, the box compares three parts with the plan's pins, in this order:
  - `payload` is the same when `pins.payload`, without a `sha256:` prefix, is a digest equal to the instrument's. An empty `pins.payload` stands for the hash of the payload folder at the study root. One `check` or `register` hashes the folder once, so every comparison, the carry (0018) and the hash `register` freezes are one value. A folder written after that hash is refused when `register` copies it.
  - `environment` is the same when `pins.environment` is a digest equal to the instrument's. A study's `host` or `""` environment names no image, so no pin matches it.
  - `inputs` is the same when every model and calibration input of the plan has a digest pin, and the set of (path, role, pin) equals the instrument's inputs.

  An empty pin never matches. A study that has not finished is pending (W-REF-001), and its comparison waits.
- **E-REF-003.** A part that differs, from a study that no deviation names, is an error at that hypothesis's `estimates`. The detail names the study's payload, its environment, its model and calibration inputs when they differ, and the parts that differ. E-REF-003 also refuses a deviation whose `from` names no study that a confirmatory hypothesis with a claim takes estimates from, and a deviation with an empty `reason`. Only a confirmatory hypothesis with a claim takes estimates, so an exploratory plan declares no deviation.
- **Deviations.** `[[pins.deviations]]` holds one entry per source study: `{ from, reason, timing }`. `from` is the study id. `timing` says when the change came, against the confirmation data: `before-collection`, `during-collection`, `after-collection-before-access`, `after-access-before-results` or `after-results`. The box records the timing and does not judge it; a reader does.
- **What `register` records.** The register record gains `deviations`: one `{from, reason, timing, differs}` per declared deviation. The box computes `differs`, the parts that differ, a subset of `payload`, `environment` and `inputs` in that order. `differs: []` means the box compared the code and found it the same. A deviation from a study that `register` no longer finds cannot be compared, so `register` refuses it and writes nothing. Before the approval prompt, `register` prints one line per deviation: `deviates from <study> in <parts, or nothing> (<timing>): <reason>`. The reason prints on its one line and in its own order: control characters, line and paragraph separators and bidirectional controls print as escapes.

Why:

- Estimates describe the code that produced them. A pipeline that changes after the pilot measures with another instrument, and the sd that sized the study need not hold for it.
- Recht and co-authors rebuilt the CIFAR-10 and ImageNet test sets by the original recipe. Models lost 3 to 15 and 11 to 14 points of accuracy, far beyond sampling error. A number holds for the setup that produced it.
- Registered reports freeze the procedure before the confirmatory data exist. The Journal of Cognition asks authors to report any change, and to consult the editors before data collection ends. Willroth and Atherton give a template that reports each deviation with its reason and its timing against the data, since a change made after the data are seen risks more bias. The five timings are theirs.
- The box computes what differs, so the record cannot understate it.

Cost: a deviation is free text, and the box cannot tell a harmless change from one that breaks the estimate. A plan that changes its code after the pilot must pin the pilot's code again or say why it does not.

Case against:

- E-REF-003 refuses only until the plan declares a deviation. One declared deviation with any reason that is not empty ends the refusal (Cost).
- The record's evidence is a change of data (Recht and co-authors, under Why). The instrument leaves data out. A borrower's confirmation data is new unless the lender is in its family (E-CNF-005, 0023). So a borrower outside a family changes the part that the cited case changed.
- The borrowed sd and icc size a study and do not enter its test (0023). A study that re-sizes corrects a wrong outside value from its own first batch (0020).
- A pin on the code does nothing for a loose estimate. A lender with 5 clusters per stratum may lend (0014, Case against).
- A lender that ran at level 0 names no image, so no pin matches it, and each of its borrowers must declare a deviation. The box's own litmus always declares one (0027).
- The box now holds a deviation table for a whole study (#70). `pins.deviations` feeds it for one case, and the code defines the five timings twice.

(written 2026-11-15)

What failure this prevents: a confirmatory study sized from an sd and an icc that other code measured, with no record that the code changed. Expected, not observed: before this record nothing compared the lender's code with the borrower's (#28). (written 2026-11-15)

Constants: none. The record fixes what is compared and what `register` records, not a value. (written 2026-11-15)

Sources:

- Allen and Mehler (2019), "Open science challenges, benefits and tips in early career and beyond", PLOS Biology 17(5), https://journals.plos.org/plosbiology/article?id=10.1371/journal.pbio.3000246
- Henderson and Chambers (2022), "Ten simple rules for writing a Registered Report", PLOS Computational Biology 18(10), https://journals.plos.org/ploscompbiol/article?id=10.1371/journal.pcbi.1010571
- Willroth and Atherton (2024), "Best Laid Plans: A Guide to Reporting Preregistration Deviations", Advances in Methods and Practices in Psychological Science 7(1), https://doi.org/10.1177/25152459231213802
- Journal of Cognition, "Registered Reports", https://journalofcognition.org/about/registered-reports
- Recht, Roelofs, Schmidt and Shankar (2019), "Do ImageNet Classifiers Generalize to ImageNet?", https://ar5iv.labs.arxiv.org/html/1902.10811
