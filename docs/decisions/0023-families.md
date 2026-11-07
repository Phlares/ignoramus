# 0023 Families of studies

Confirmatory studies declared together, before any of them runs, form a family. Members may reuse each other's confirmation and reserve data. Each member keeps its own stream slot, its own level and its own verdicts. A confirmatory study may also take selection data from data an earlier plan used.

Decision:

- **Declaration.** The first study, the root, holds a top-level `[family]` table with `members`, a list of spec paths from the study root. The family's name is the root's study id. Members declare nothing. The order has no meaning. Membership is independent of `follow_on` routes: routes say what runs next, a family says what may share data.
- **Check on the root.** `check` reads every member spec with the same rules as a standalone check, plus the family rules:
  - E-FAM-001: root and members are confirmatory and use the same log.
  - E-FAM-002: every member id is distinct, unregistered and in no other family. No member holds a `[family]` table.
  - E-FAM-003: every input of every member, of every role, is pinned by SHA-256. Its data is at the study root or in the store.
  - E-FAM-004: no member's confirmation or reserve data holds the data of a selection input of any family study, root included. Exact and normalized hashes are compared.
  - E-FAM-005: the plan stream holds a free slot for the root and every member.
  - W-FAM-001: a member value that waits on an unfinished study is allowed and listed. It resolves at the member's own registration (0014).
  - W-FAM-002: a spec reachable through `follow_on` shares confirmation or reserve data with the study and is not in its family. The warning comes while the family option is still open.
- **Register on the root.** `register` imports every member input into the store, as it imports its own. It signs one plan-log leaf of kind `family`, labelled `family:<root id>`, after the root's plan leaf. The root's register record holds the lock: per member, the study id, spec path, spec hash, payload tree hash, and each input's role and SHA-256. It lists no absolute path (0012). The leaf commits to the lock's hash.
- **The lock.** After the root registers, a member registers and checks only as locked: its spec hash, payload tree hash and input pins equal the locked ones (E-FAM-006). A locked member registers only as locked, in every mode. The check-time lock rule runs for confirmatory plans, and `register` enforces it for every mode. To change a member, register the changed study under a new id, as a study outside the family.
- **The stream.** A member's slot opens when the member registers, as any study's does. Its level is 0.005 × (D + 1), with D the discoveries closed before the slot opens. The plan stream has 10 slots. A study that is not an unopened member may open a slot only if, after it, the slots left cover every unopened member of every registered family. A reserved slot that is never opened stays reserved and never counts a discovery. The test stream never runs out, so it reserves nothing. Every study of a family uses one log.
- **The freshness rule.** E-CNF-005 no longer applies to `selection` inputs. Earlier uses of any role still count against later confirmation and reserve inputs. E-CNF-004 is unchanged. Uses by members of the study's own family do not count, except where the member's confirmation or reserve data is a family study's selection data (E-FAM-004).
- **The data that picked a condition.** A confirmatory study may not confirm on the selection data of a study that a `{ from = X }` param takes its condition from (E-CNF-007). The rule is general: it applies to members and to every other confirmatory study. The box reads X's selection data from the inputs record X froze at its first run and the roles in X's stored plan, and compares exact and normalized hashes. It runs at `check` once X has finished, and at `register`, which waits for X.
- **After the lock.** For an outsider, the family leaf counts as a use of every member's confirmation and reserve data: an outside study cannot confirm on it, and may still select on it. For a plan-log member, only uses recorded before the family leaf count. A later outside use, of any role and in any mode, cannot have shaped a locked member, so it never makes the member unregistrable. The one exception is a use that selected a condition the member takes through a `{ from = X }` param: E-CNF-007 refuses the member if it confirms on that selection data. A root's `release` leaf does not free reserve data that a member's lock holds.
- **The ways forward.** E-CNF-005 lists them with their costs. If the earlier study is not registered yet, add this study to its family: every member is locked, and each takes a stream slot. Run this study as exploratory on the same data: no claim. Give it fresh data. If it is already a member of the earlier study's family, nothing is needed.
- **Where a root confirms.** A root may confirm on data a member selects on. A member's selection result reaches nothing the root tests, because the root registers first and cannot wait on a member's values.
- **Views.** `check` and `review` show the family: members, locks and the slots left and reserved.

Limits:

- If a root's `register` stops after it signs its family leaf and before its register record lands, the family leaf has no lock: it is an orphan. Its data stays blocked for outsiders, and its root's id cannot register again. New data is the way out. Other families are unaffected: a member's family is read only from the leaf of the root whose lock lists it.
- For a family on the test log, a later plan-log use of a member's data still counts against that member. Only plan-log families get the rule that a later outside use never counts.
- A root reserves slots, and `register` reads the slots left before it takes the stream lock. A concurrent register between the root's slot opening and its register record landing can take a slot that a member needs.
- A register record that holds a family lock cannot be read by an older box binary.
- A member sized by `estimates = { from = X }`, where X ran after the lock on shared data, takes its sd and icc from that data. This affects only the power gate, not the test: the e-value does not read the estimates.
- The box adds no gate: members are confirmatory whatever an earlier member's verdict is. It has no amendment path and no withdraw command. A study that a trigger creates depends on a result, so it is never a member.

Why:

- The e-LOND procedure gives slot t the level α_t = α × γ_t × (D_(t−1) + 1) and controls the false discovery rate under arbitrary dependence (Xu and Ramdas). The proof needs only E[E_t] ≤ 1 for each null slot. When slot t rejects, the rejections R number at least D_(t−1) + 1, so 1{E_t ≥ 1/α_t} / max(R, 1) ≤ α × γ_t × E_t, and the sum over slots is at most α. Shared data do not enter. The bound holds also when D counts only the discoveries closed before a slot opens, since D_open + 1 ≤ R on a rejection.
- Four conditions keep E[E_t] ≤ 1 and the sum bounded:
  1. No member's hypothesis, formula, payload or data is chosen from data its e-value uses. The lock at the root's registration, before any family data is scored, ensures it. An online hypothesis may use past data, but its e-value needs fresh data (Fischer and Ramdas); specifications fixed before the results are the accepted form (Pima trial).
  2. The family is bounded when declared. Choosing which of many locked candidates to register is a selection, and control of the error rate needs the addition planned (added-arm error control). Reserved slots fix the family's size.
  3. No member's evidence comes from data that picked its condition (E-FAM-004, E-CNF-007). A condition picked on some data looks better on that data (FDA 2019).
  4. γ is fixed before the stream starts (0013).
- Selection reuse rests on the same ground as condition 1: the hypothesis and design may come from any past data, and the e-value needs fresh data (Neufeld, Perry and Witten). A condition picked on old data does not bias a test of that condition on fresh data.
- A root may confirm on a member's selection data, because a member's selection result reaches nothing the root tests. Condition 3 holds in the other direction, where it matters.
- There is no gate. Gatekeeping lets one secondary claim ride on a primary claim because one α covers the family (FDA 2022). Here each member has its own slot.
- Locks are strict. Registered-report journals require withdrawal and a new registration for substantial changes. An amendment path needs a person to judge "minor"; here rules decide.
- A family is the safe form of a benchmark: the methods are fixed before any scoring, and all use the same data. An open benchmark, where new methods arrive after results are public, stays exploratory or uses fresh data. The box cannot check that a later method was not shaped by earlier results.

Cost:

- No source states this family rule as a practice. It combines the e-LOND theorem with the fresh-data rule for data-chosen hypotheses.
- The box controls the false discovery rate over the stream, not the familywise error rate over the family. With shared data, false claims tend to come together.
- A family takes several of the plan stream's 10 slots at once, and keeps them whether or not its members run.
- A claim tested on data another claim used reaches "confirmed", never "replicated" (Junk and Lyons).
- One payload folder serves a whole family, so one payload fix after the root registers removes every member.
- A member gate that can run only at the member's own registration, because its values wait on an unfinished study, is a risk the lock accepts. A member that fails a gate then is refused and never opens its slot, which stays reserved and unused. Size its reserves generously.

Sources:

- Xu and Ramdas, e-LOND, online false discovery rate control with e-values, https://arxiv.org/abs/2311.06412
- Fischer and Ramdas, "Admissible online closed testing must employ e-values", https://arxiv.org/abs/2407.15733
- Neufeld, Perry and Witten, one recipe for inference after selection: choose the target on selection data, then infer conditional on that choice, https://arxiv.org/abs/2604.09779
- FDA (2019), "Adaptive Designs for Clinical Trials of Drugs and Biologics", https://www.fda.gov/media/78495/download
- FDA (2022), serial and parallel gatekeeping, "Multiple Endpoints in Clinical Trials", https://www.fda.gov/media/162416/download
- Added-arm designs and familywise error control, https://ar5iv.labs.arxiv.org/html/1902.05336
- Pima trial, specifications fixed before the results, https://ar5iv.labs.arxiv.org/html/2210.02794
- Journal of Cognition, "Registered reports": substantial changes need withdrawal and a new registration, https://journalofcognition.org/about/registered-reports
- Junk and Lyons, overlapping data are not independent confirmation, https://arxiv.org/abs/2009.06864
