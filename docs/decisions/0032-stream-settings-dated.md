# 0032 Stream settings: a dated record

Date: 2026-11-15. The settings were decided on 2026-10-29 and have been in the code since 2026-10-31 (0013, "Stream settings"). This record adds their date, their basis and the rule for change.

The plan stream gives each confirmatory study its level. Its two settings sat in 0013 with no date, no stated basis and no rule for change. A full stream refuses the next study and says nothing more.

Decision:

- **Box constants.** alpha = 0.05 and 10 slots. A slot's level is 0.005 × (D + 1), where D counts the discoveries closed before the slot opened (0013). Neither is a spec value.
- **Basis of alpha.** 0.05 is the false discovery rate over every claim the box confirms. e-LOND keeps it under any dependence between the studies.
- **Basis of the 10 slots.** The weights are spread evenly over a fixed number of slots, 1/10 each. With 10 slots the first study's level is 0.005. That is the default level for a new claim that brings no justified level of its own.
- **Rejected.**
  - A weight sequence with no bound, 1 / (t (t + 1)): study 1 gets 0.025 and study 10 gets 0.00045.
  - A stream alpha of 0.005: each study gets 0.0005 and needs an e-value of 2,000. E-value tests have low power at small counts.
  - Not chosen: a cap of 0.005 on every level, whatever the discoveries. That is control of the family-wise error rate over the 10 slots.
- **Frozen per stream.** A signed `stream.config` record fixes alpha and the weights before a stream's first slot. A box reads a stream only when that record holds the box's own alpha and weights. The first box home that registered a confirmatory study signed its record on 2026-11-10.
- **Rule for change.** The slot count changes only with the run budget, never with outcomes. A change spreads only the budget not yet spent. The order of studies is fixed before results. New constants need a new decision record with a date and a basis. No command changes a stream's settings today.
- **The card names the source of the level.** The card prints a study's level with the stream settings, the slot and the count of discoveries before it. A reader then sees a rule for many studies, not a threshold chosen for one. Not built yet.
- **A full stream refuses.** `check` reports E-STR-001: "Slot 11 is beyond the 10 slots of the planlog stream." `register` refuses before it writes the plan leaf. The box offers no reset and no second stream. Two tests hold this: `check_refuses_a_plan_beyond_the_tenth_slot` and `a_slot_lost_at_the_prompt_is_refused_before_the_plan_leaf`.

Evidence:

- The first study family takes 4 of the 10 slots: one study has run and three are planned. Six stay free.
- Every registration on the plan log opens a slot. A study that registers and cannot finish still spends its slot.

Why:

- A constant of the analysis that carries no date cannot be shown to have stayed fixed.
- The level is not a spec value because it depends on the box's history. No author can type it (0013).
- A bound on the number of tests, fixed in advance, gives every test a higher level than a sequence with no bound. The gain is large below 100 tests.

Case against:

- A stream is signed at its first slot. This record changes nothing for a stream that exists.
- No failure in the first study stands behind this record. A later pass that removes mechanisms will question it.
- The control is of the false discovery rate, not of the family-wise error rate. Under shared data, false claims tend to come together.
- Ten slots with no reset stop the eleventh confirmatory study of a box. Once all ten are open, no budget is left to spread.

What failure this prevents: a change of alpha or of the slot count that no reader can date, and a full stream that leaves the author no stated next step.

Open:

- The rule for a full stream. The source on bounded testing allows new weights over the unspent budget when the bound changes before it is reached. Nothing is decided for a stream that is already full.
- A new box home starts a new stream at slot 1 (#87). A rule for a full stream must not make that the answer.

Sources:

- Xu and Ramdas, "Online multiple testing with e-values": e-LOND keeps the online false discovery rate at or below alpha under arbitrary dependence. https://arxiv.org/abs/2311.06412
- Bounded online testing: with an upper bound on the number of tests fixed in advance, the bounded rules have uniformly higher power; a later change of the bound spreads only the unspent budget. https://ar5iv.labs.arxiv.org/html/2208.11418 and https://ar5iv.labs.arxiv.org/html/1809.07292
- Lakens and others, "Justify your alpha": no blanket level; state the reason before the data. https://eprints.whiterose.ac.uk/id/eprint/128599/8/Justify%20Your%20Alpha.pdf
