# 0024 The spawn engine

A trigger watches the selection data during a run. When it fires, the box runs the children the trigger declared, on the confirmation data. This build is for exploratory studies only, on either log. It makes no confirmatory claim.

Decision:

- **The trigger.** A trigger states what it watches with the terms of a hypothesis (0013): `outcome`, `contrast`, `claim` and `statistic`, and `strata` if needed. It adds `threshold_e`, which must be above 1, and `spawns`. The statistic fixes the bet, so no betting rule can be chosen after the data. A trigger contrast may name `selected`, the selection stage's pick. It may not name the reference.
- **The children.** `spawns` lists, per operator, the values to try. An `enum` value is in `range`. An `int` value is an integer in `[lo, hi]`. A `float` value lies in `[lo, hi]`. An operator's `acts_on` names a declared condition or `selected`. A child is the `acts_on` condition with one operator set to one value. Its params are the parent's resolved params plus that value, which the box passes to the payload as given. A value equal to the parent's value spawns nothing.
- **One child identity.** A child's identity is its `acts_on` condition, operator and value. Numbers compare as numbers, so 64 and 64.0 are one child. JSON types stay distinct: the string "64" and the number 64 are two children. A child that two triggers or two spawn entries declare runs once. Its rows name the first trigger that admitted it. `check` counts it once in the worst-case cost and warns on the overlap.
- **Depth 1.** Children carry no triggers. A spec with a declared trigger needs `mutations.depth_cap` of 1 or more (E-MUT-001).
- **Selection stage.** The selection rows run before the confirmation rows, as in 0009. A spec with triggers may hold selection inputs without `[design.selection]`. The selection rows then run the conditions the triggers' contrasts name.
- **Disjoint data.** In any spec with triggers, whatever its mode, the selection data and the confirmation data must be disjoint. E-CNF-004's comparison applies to it.
- **Firing.** After the selection rows, the box evaluates each trigger in spec order. It builds one observation per selection cluster (0013), orders them with a seed from `mutations.seeds.spawn` and the trigger id, and runs the e-process with a stop at `threshold_e`. The trigger fires when the stopped value reaches the threshold. An incomplete selection unit in scope, or a refused or non-finite observation, makes the value 0, so the trigger does not fire.
- **The pick.** The box makes the pick only when a trigger's contrast or a child's `acts_on` names `selected`. Otherwise triggers evaluate without a pick. If a trigger needs the pick and no selection unit is complete, its record says not fired, reason `no-pick`, and the run continues.
- **Admission.** The box admits the children of a fired trigger in declared order, up to the budget. It works from the spec, not from measured time. A child is admitted if the estimate of the non-child rows, the children admitted before it and its own rows stays within the budget, with the run gate's margin. A time cap gives the same children on every machine. A money cap depends on the machine's rate, so the same spec gives the same children only at the same rate. A child that does not fit is skipped, with the reason `budget`.
- **The record.** The box signs one `trigger` record per trigger before any child row runs. It holds the trigger id, the stopped e-value, the threshold, whether it fired, the children admitted and the children skipped with reasons. The plan stream gains no record kind. The study log gains three kinds: `trigger`, `spawn-stop` and `child-estimate`. All children are scheduled up front, after the confirmation rows, in the confirmation stage's order. A child row runs only if a signed trigger record admits it. The reference rows serve the children, so the comparison is concurrent.
- **Overrun.** A child stops only on a real budget overrun, when the cap is reached or the next row would cross it. Any other pause, such as a meter fault, pauses the run as it does for a parent row. A stopped child never blocks the parent: the study finishes, and the parent's hypotheses get their verdicts.
- **Child estimates.** Each child gets an estimate per outcome: its paired values against the reference, as a mean over clusters, with the cluster and unit counts. A unit counts if any repeat pairs, as in the main estimates. A stopped child is estimated from the rows that ran. Child estimates are a separate record kind, not 0014 estimate records. A later study cannot borrow them through `estimates = { from = ... }`.
- **Old shape.** A stored spec whose triggers have the old shape (`id`, `metric`, `threshold_e`) still parses. The trigger is inert, and `status` says so.
- **Rules.** The E-MUT rules refuse an unknown outcome, contrast condition or operator, a value outside an operator's range or of another type, a trigger with no selection inputs, and a confirmatory spec with triggers. W-MUT-001 warns on a repeated child. W-MUT-002 warns when the worst-case cost of the children exceeds the budget, and names the children a run would skip.
- **Views.** `status` shows each trigger and its children. `review` lists the child set and its worst-case cost. `report` lists children with their estimates. Children are blinded as conditions are.

Limits:

- Exploratory studies only, on either log. A confirmatory spec with triggers is refused. Two gates come first: a null-tree simulation of the declared spawn policy, and a rules-based, blind evaluation of the trigger decision. The decision reveals the interim effect to anyone who knows the rule and sees which children ran. Blind evaluation keeps the record from people until the study ends.
- Depth is 1.
- Child estimates are not borrowable. When they become so, borrow the spread only, never the effect, since a child picked from a searched set has an inflated effect.
- Trigger records are visible to people. That is fine for an exploratory study. It is a blinding question for a confirmatory one.
- A spawn study's selection and confirmation data become used for every later confirmatory study (E-CNF-005, 0023). Do not run a spawn study on data planned for later confirmatory work.
- Children earn "signal" at most. A searched-over set of variants inflates the best score, so confirmation needs fresh data.
- A money cap makes admission depend on the machine's rate.

Why:

- The tree split of 0009 is valid when the set of hypotheses that ran does not depend on the data their e-values come from. Triggers read selection data. Children are tested on confirmation data. The disjoint-data rule keeps the two apart. Picking a condition and testing it on the same data inflates the type I error (FDA 2019). Selecting on training data and testing only the pick is the sound form (Eggensperger et al.).
- A trigger-only selection stage follows one recipe: declare the selection data and the inference data up front, then infer conditional on the choice (Neufeld, Perry and Witten).
- Admission from the spec makes the rule algorithmic and binding. The same spec gives the same children, and the trigger record is signed before any confirmation row. An adaptation keeps its error control only under a binding, algorithmic rule, and a planned adaptation fires on its written trigger without further approval (Zebrowska et al.; adaptive design reporting guidance).
- A child never blocks the parent. A non-binding stop costs nothing when the final test runs at its full level, so the parent's tests keep their level (multi-arm multi-stage designs; FDA 2019).
- A missing case is not a passing case. An incomplete unit gives the trigger value 0. Spawning is optional, so failing to spawn is the safe side.
- Child estimates stay out of the 0014 records so that the "finished" rule and the estimate keys do not change.

Cost:

- A spawn study uses its data for later work. Its selection and confirmation data cannot confirm anything later.
- Admission from the spec ignores real speed. A child may be skipped that would have fitted, and a child may stop on overrun that the estimate admitted.
- Selection rows run for every candidate the triggers name, so a study with triggers costs more than one without.
- Child results are descriptive. They add no claim.

Sources:

- FDA (2019), "Adaptive Designs for Clinical Trials of Drugs and Biologics", https://www.fda.gov/media/78495/download
- Eggensperger et al., select on training data, test only the pick, https://ar5iv.labs.arxiv.org/html/1705.06058
- Neufeld, Perry and Witten, one recipe for selection then inference, https://arxiv.org/abs/2604.09779
- Blinded sample size re-estimation needs a binding, algorithmic rule (Zebrowska et al.), https://www.ebi.ac.uk/europepmc/webservices/rest/PMC4851240/fullTextXML
- Reporting planned and unplanned adaptations, https://www.ebi.ac.uk/europepmc/webservices/rest/PMC5830330/fullTextXML
- Multi-arm multi-stage designs, non-binding stops and the final level, https://www.ebi.ac.uk/europepmc/webservices/rest/PMC4930581/fullTextXML
- The winner's curse of a best-of-many pick, https://arxiv.org/html/2605.05973
- Sample size for replication after a winner's curse, https://ar5iv.labs.arxiv.org/html/1010.5095
- Borrowing variance from earlier tasks for sizing, https://link.springer.com/article/10.1007/s10791-015-9273-z
