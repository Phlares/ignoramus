# Study card: s1

## Header

- Study: s1
- Family: none
- Mode: confirmatory
- Blind: no
- Isolation level: 2
- Registered: 2026-11-14T20:21:09.221Z (plan hash sha256:0a8dd75d8ea12c0b0fa61d8181b7f4644ba0e99710894279075edc760f1ad0df)
- Outside time stamp: not in the records
- Finished: 2026-11-14T20:21:09.383Z
- Records verified: 87

## Claim

- Population: every text file of the four strata
- H1 (ppl-ratio): claim below 0.039221; mean clipped to \[-0.693147, 0.693147\]; level 0.005; pass rule: e-value at least 200.0 in code, article

## Verdicts

- H1: not shown (basis test, level 0.005, e-value 3.100)

"Not shown" means the registered test did not reach its pass e-value. It does not show the claim is false.

## Claim label

- H1: label not computed

## Results

| Hypothesis | Group | Outcome | Estimate | Change | Interval | E-value | Clusters | Units | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| H1 | code | ppl-ratio | 0.02870 | +2.91% | below: no grid margin passes (inverted registered e-process; grid: steps of 0.0001 from the registered margin, searched from 0.028621 to 0.693147; post hoc) | 3.100 | 4 | 20 | registered |
| H1 | article | ppl-ratio | 0.02870 | +2.91% | below: no grid margin passes (inverted registered e-process; grid: steps of 0.0001 from the registered margin, searched from 0.028621 to 0.693147; post hoc) | 3.100 | 4 | 20 | registered |
| H1 | all | ppl-ratio | 0.02870 | +2.91% | no interval |  | 8 | 40 | descriptive |

## Sensitivity

- H1: power 0.98 at 0.01980 (target 0.80)
- H1: toward margin 0.039221: 0.98, 0.81, 0.33, 0.04, 0.00 at 0, 25, 50, 75 and 100%

| Hypothesis | Group | Estimate | Margin | Nearest below | Nearest above |
| --- | --- | --- | --- | --- | --- |
| H1 | code | 0.02870 | 0.039221 | mean 0.02466, power 0.81 | mean 0.02951, power 0.33 |
| H1 | article | 0.02870 | 0.039221 | mean 0.02466, power 0.81 | mean 0.02951, power 0.33 |

## Figures

- ![H1: code: estimate 0.02870 (+2.91%); article: estimate 0.02870 (+2.91%). Registered margin: below 0.039221.](figure-h1.svg)

## Setup

- Input list: 8 files, list SHA-256 e5c522272fde8c9d554fc18b865dbff94bdfe688baa365c38169f44891655c97, file data.sha256
- 2 inputs without a digest
- Input folder: holds exactly the input list
- Observations: 16 rows, SHA-256 26118cd324e5bb03652f87664f3e2045cbc30924fd3879c08694a926481c18a2, file observations.csv
- Data grade: not in the records
- Payload: cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd
- Environment: efefefefefefefefefefefefefefefefefefefefefefefefefefefefefefefef
- Model input model/m.bin (model): 13e35c44395f6cade670c076230362267c5288989d3498e207877d62b859a6d8
- Seed root (plan hash): sha256:0a8dd75d8ea12c0b0fa61d8181b7f4644ba0e99710894279075edc760f1ad0df
- Row seeds: 0 of 0 derive from the root by: SHA-256(plan hash bytes \|\| "row/\<index\>"); a recheck copy keeps the seed of the row it repeats
- Launcher: cloud-hypervisor v41.0
- Agent SHA-256: abababababababababababababababababababababababababababababababab
- Virtual CPUs: not in the records
- Row memory cap (MiB): 2304
- Values the box defaulted: \[\]
- File gaps.md: absent
- File decisions.md: absent
- File deviations.toml: 37b4026807fe5c96bcacafec1a782e37abc01ea0de834218d37cc3b5f65fab26
- File CITATION.cff: absent
- File prereg/statement.md: 41223cbbb20ae697e44152d58bac670e18fa614caf3b68c827bc1733d80feacd
- Outside files pinned at commit 0123456789abcdef0123456789abcdef01234567, folder "studies/example/"

## Deviations

| Mark | Type | Reason | Timing | Original | Justification | Impact | Source |
| --- | --- | --- | --- | --- | --- | --- | --- |
| unsigned deviation | analysis | the payload crashed on one seed | after-access-before-results | five seeds | one seed hung | four seeds reach the test | notes.md:12 |
| unsigned unregistered | reran the extractor | disk full | during-collection |  |  |  | sha256:ab |

## Surprises

- Retried attempts: 0
- Box versions: 0.1.0

## Cost

- CPU seconds: 0; budget 1 CPU hours
- Wall seconds: 0; budget 1 wall hours
- Money (USD): not in the records; budget 0
- Completed rows: 80; attempts charged the row estimate for an unread CPU count: 0
- Telemetry: 0 of 80 rows hold it
- Host CPU models: none (0 read, 0 unavailable)
- Host CPU ticks summed over rows, seconds (rows that ran at the same time each count the same host ticks): {}
- Cgroup counters: {}
- Peak memory, bytes: none read

## Runs

- confirmation: 80 rows ok
- Run sessions: not in the records (see events.jsonl)

Executions recorded nowhere cannot be listed.

## Re-run recipe

Command:

```
ignorarium run s1 --max-level 2
```

- Requires: a box home where the study is registered
- Requires: the study folder as the working directory
- Requires: the box version recorded in \`box\_version\`
- Isolation levels held: \[2\]
- Box version: 0.1.0
- Box binary SHA-256: not in the records
- Card builder: version 0.0.2, binary SHA-256 cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd (not the box version that ran the study)
- Registered box: not in the records
- Bundle DOI: not in the records
- Manifest roots: spec.toml, spec.json, spec.hash, salt, schedule.json, in/, payload/
- Roots the folder lacks: spec.json, spec.hash, salt, schedule.json, in/, payload/
- Manifest: 1 files, list SHA-256 53c7939fa1b6a82466144408e7ed4dcdc51ace61a01a933da54727d670c884db

manifest.sha256 names the files of the study's folder in the box home. Check them there:

```
(cd <box home>/studies/s1 && sha256sum -c) < <card folder>/manifest.sha256
```

## Citation

BibTeX: not in the records

CITATION.cff: not in the records

## Author readings

Not yet read by the author. The slots are in readings.md.
