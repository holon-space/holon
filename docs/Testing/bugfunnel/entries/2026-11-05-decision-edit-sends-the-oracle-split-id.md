---
id: 2026-11-05-decision-edit-sends-the-oracle-split-id
date: 2026-11-05
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  A keystone decision edit after a split dispatched its planned ops with the
  oracle's synthetic `block::split-N` id, so the write authority refused a
  legal edit and inv-shape-gate-refuses-illegal-writes went red with no
  product defect behind it.
---

## Bug
The composed keystone went red in a keystone smoke run (seed
2333482106343229594, excerpt
docs/Testing/known-red-evidence/decision-edit-sends-unresolved-split-id/keystone-smoke-excerpt.log.zst): `inv-shape-gate-refuses-illegal-writes`
reported "the resulting decision is legal, yet the write was refused:
block::split-13 is not in the write authority". The failing draw has 28
transitions under the wiring {Turso}, no sync, {ActionEngine}; transition 26
is an `EditDecisionSubtree` outdent of a block that an earlier `SplitBlock`
created.

## Root cause
`EditDecisionSubtree::apply_to_sut`
(crates/holon-integration-tests/src/pbt/transitions/edit_decision_subtree.rs:682)
and `EditDecisionPairHeld::apply_to_sut`
(crates/holon-integration-tests/src/pbt/transitions/edit_decision_pair_held.rs:130)
send the ops the oracle planned. Those ops name blocks in oracle space. A
split tail exists in the oracle only as the synthetic `block::split-N`; the
SUT minted its own id for it. `SutShapeEdit::apply_shape_edit` and
`apply_held_shape_pair` in
crates/holon-integration-tests/src/pbt/frontend_slice/components.rs passed
the ops to the engine without the `IdResolver` pairing, so the store was asked
to write a block it never had. The store's refusal was correct.

## Missing piece
The shape-edit capability of the headless component did not resolve the id
params of a planned op, unlike the op-dispatch writer, which resolves every id
through `resolve_sut_id` (crates/holon-integration-tests/src/pbt/op_write_cap.rs:130).

## Remedy
`HeadlessFrontendComponent::resolve_op_ids` resolves the `id`, `parent_id`
and `after_block_id` params of every planned shape op through
`resolve_sut_id` before dispatch; with no resolver wired it fails loudly on an
oracle-only id. The unshrunk failing draw is the hand-authored case
`decision-edit-after-splits-reaches-the-sut-by-its-minted-id`
(crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl).
The shrunk sequence loses the failure, so the case is kept unshrunk.
