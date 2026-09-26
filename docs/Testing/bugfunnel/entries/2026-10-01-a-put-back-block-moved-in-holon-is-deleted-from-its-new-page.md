---
id: 2026-10-01-a-put-back-block-moved-in-holon-is-deleted-from-its-new-page
date: 2026-10-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A put-back block that the user moves in Holon to another page is deleted from that page, with no disclosure, when the copy lets go and the deletion stands.
---

## Bug
Found by the D229 round-9 verifier (lane d229-move), probe VP-C: after the put-back, the user moves the child in Holon under `notes-1` (`Notes.org`). When `Overview.org` lets go of the child, the child is gone from the store and from `Notes.org`, the sidecar is empty, and nothing names it. Report: `lane-logs/d229r9v-verify.md`, Defect B.

## Root cause
`put_back_fingerprint` (`crates/holon-filesystem/src/file_sync_controller.rs`) rendered the block with its parent forced to the copied heading, so a move left the hash unchanged; `apply_standing_deletions` then deleted the block at its current owner.

## Missing piece
The keystone model has no Holon move of an undone member; no org-suite test moved a put-back block.

## Remedy
Fixed in round 10: the fingerprint includes the block's stored parent, so a move ends the deletion and the block stays on its new page, with `DeletionEndedByEdit`. Org-suite `a_put_back_child_moved_in_holon_to_another_page_stays_when_the_copy_lets_go`; red `lane-logs/d229r10-red-vp.log`, green `lane-logs/d229r10-green-vp.log`, sabotage red `lane-logs/d229r10-sabotage.log`.
