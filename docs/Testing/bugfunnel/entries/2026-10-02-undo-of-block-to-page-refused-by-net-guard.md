---
id: 2026-10-02-undo-of-block-to-page-refused-by-net-guard
date: 2026-10-02
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  Undoing a block-to-page conversion of a block that holds rule machinery stopped part way:
  the net gate refused the inverse move of the rule's action block, leaving the undo half applied.
---

## Bug

`BlockToPage` of a block with rule machinery (the journals auto-create rule), then
`UndoLastMutation`, failed with `undo: composite inverse op 2 of 4 ('move_block' on 'block')
failed — stopping (partial undo, earlier inverses already applied): ... ADR 0032 net-guard
refusal: block.move_block — re-homing a rule's action block breaks the rule`. Found by the
Inc 7 r2 attribution probe (known-red row `block-to-page-undo-refused-by-net-guard`, registered
in the unlanded Inc 7 lane); lane row3.

## Root cause

The forward child moves of `convert_block_to_page` and `merge_blocks` carry
`confirm_break: machinery_containment` (operation_engine.rs, the child/dedupe move loops). The
inverse `move_block` is built by the provider from captured placement and carries no
confirmation, so the undo replay was judged by the net gate unconfirmed.
`dispatch_constituent` and `dispatch_merge_constituent` returned that inverse unchanged.

## Missing piece

No hand-authored case undid a block-to-page of a machinery-bearing block; `merge_blocks` is not
a keystone transition, so its undo has no case at all.

## Remedy

`carry_confirmation` (operation_engine.rs) copies the forward's `confirm_break` onto its
inverse; both constituent dispatchers use it, so convert and merge share the fix without an op
allowlist. Case `undo-of-block-to-page-with-rule-machinery-restores-it` in
`hand-authored-regressions/keystone.jsonl`.

Evidence: red `lane-logs/row3-red.log` (net-guard refusal), green `lane-logs/row3-green.log`
(9 passed), teeth `lane-logs/row3-teeth.log` (fix removed, same refusal; restored by cp,
sha256 identical). Open: merge undo is fixed by the same helper but has no regression case until
a merge transition exists.
