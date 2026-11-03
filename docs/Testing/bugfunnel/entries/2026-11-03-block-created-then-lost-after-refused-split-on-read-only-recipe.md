---
id: 2026-11-03-block-created-then-lost-after-refused-split-on-read-only-recipe
date: 2026-11-03
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  In a draw where a split on a read-only cooklang block is refused, a block the
  store just created is missing from block_raw at the next reconcile tick.
---

## Bug
The composed keystone went red in a random draw (seed 3586274752803461974)
during the settle A/B population runs. First panic, harness.rs:2205:
"per-tick reconcile: one synthetic per minted real id (syn=[block::create-5,
block::split-10], real=[]) ... this tick RETIRED [(block::create-5,
block:<uuid>)] and the SUT LOST [block:<uuid>] from block_raw". Just before
it, `block.split_block` failed with "cooklang is a read-only format". The
shrink then reports `inv-shape-sim-matches-authority` (block.split_block,
children of block:kd-1) and `inv-block-content` (ref "r\n- ftxo", store
"Mr\n- ftxo"). The same seed is red on the base tree (24ef65bd) with the same
first panic and the same signatures, so the settle change did not cause it.

## Root cause
Not found. Not root-caused in the lane that found it: the failing path is in
the product write path (split refusal on a read-only home, create/retire
reconcile), outside that lane's files.

## Missing piece
The keystone can generate the sequence, and its invariants fire, but no gate
run had drawn it: the gate's draws did not reach a refused split on a
read-only recipe followed by a create in the same tick.

## Remedy
Open. Replay `PROPTEST_RNG_SEED=3586274752803461974` on the composed keystone
and root-cause the first panic (harness.rs:2205) before the shape-sim and
content signatures, which follow from it in the shrink.
