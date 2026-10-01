---
id: 2026-10-01-a-stable-id-lookup-inside-a-batch-misses-a-node-revived-by-revert-or-move
date: 2026-10-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Inside an open write batch, a stable-id lookup after `revert_to` or after a
  raw move that revives a deleted node answered "absent" (release) or panicked
  (debug), although the tree held the id.
---

## Bug
Found by the adversarial verifier of the stable-id index (deep-vault lane,
`lane-logs/dv-i13-verify.md`, probes p13 and p3). The index hears tree changes
at commit; inside a batch it knew only the nodes `write_stable_id` noted. A
miss is authoritative, so in a release build the caller would create a second
node for the id.

## Root cause
`StableIdIndex::lookup` (`crates/holon-loro/src/stable_id_index.rs`) trusted a
miss whenever the touched set was drained, but `revert_to` and a move of a
deleted node's child put nodes back without a note; their events arrive only
at the batch's commit.

## Missing piece
The index PBT's batch alphabet had create, rewrite and delete only — no revert
and no revive inside a batch.

## Remedy
A lookup that misses while the doc has uncommitted ops scans the tree in every
build profile and notes the carriers it finds (`open_batch_scans` counts
them; cost tests pin 0 for batched creates). `WriteTxn::revert_to` makes the
index rebuild at its next lookup, so a revert inside a batch also resolves
duplicates exactly. Left disclosed: after a RAW move that revives a deleted
duplicate inside a batch, a hit may name the larger carrier until the commit;
no production API moves a deleted node. Pinned by the PBT's `Revive` and
`Revert` batch ops and the replays `a_node_revived_inside_a_batch_is_found`,
`a_node_reverted_inside_a_batch_is_found`,
`a_duplicate_reverted_inside_a_batch_answers_its_smallest_carrier`. Red log
`lane-logs/dv-fix-red-2.log`, teeth `lane-logs/dv-fix-teeth-driver.log`
(t2–t4).
