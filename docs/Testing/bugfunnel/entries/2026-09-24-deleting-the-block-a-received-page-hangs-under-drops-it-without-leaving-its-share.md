---
id: 2026-09-24-deleting-the-block-a-received-page-hangs-under-drops-it-without-leaving-its-share
date: 2026-09-24
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Deleting the block a received shared page hangs under removed the page's
  mount without leaving its share, so the share stayed loaded, nothing was
  disclosed and the page's rows were orphaned.
---

## Bug
Found by a verifier probe of the overlay page-share lane (D198.a): on the
recipient, `LoroBackend::delete_block("block:root-b")`, where the received page
was accepted under `root-b`, returned `Ok(())`. The mount was gone, the shared
doc stayed registered, no `left-shared-page` condition was raised, and
`block:shared-page`'s SQL row stayed under a parent that no longer existed.
Production reaches it through `BlockCellRegistry::delete_entity` (the
`delete_subtree` cell route) and through both ingest `delete_in_tree` seams.

## Root cause
`LoroBackend::delete_block` classified only the node it was asked to delete
(`received_share_of` on `id`); `tree.delete` then removed the whole global
subtree, mount included. Every leave call site (`delete`, `delete_subtree`,
the two `delete_in_tree` seams) asked the same one-node question.

## Missing piece
The two-instance slice had no transition that deletes an ancestor of a mount:
`DeletePlacedRoot` only deletes the placed page itself.

## Remedy
`LoroBackend::received_pages_at` (crates/holon-loro/src/loro_backend.rs:2703)
finds every received page at or under a block by walking the global subtree for
recipient page mounts. `leave_received_pages` (:2752) leaves each through the
one exit, and `delete_block` / `delete_blocks` refuse to remove a block that is
or holds one (:2768). The op routes leave first, delete second and declare the
op irreversible. New transition `DeletePlacementParent` (receiver files the
page under a new page and deletes that page's subtree) plus the chain test
`deleting_the_block_a_received_page_hangs_under_leaves_its_share` in
`two_instance_composed_pbt.rs`; unit pins in `loro_share_backend.rs`
(`deleting_the_block_a_received_page_hangs_under_leaves_its_share`,
`a_plain_delete_of_the_block_a_received_page_hangs_under_is_refused`).
