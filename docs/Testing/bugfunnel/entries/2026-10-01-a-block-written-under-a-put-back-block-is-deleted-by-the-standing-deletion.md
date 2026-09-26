---
id: 2026-10-01-a-block-written-under-a-put-back-block-is-deleted-by-the-standing-deletion
date: 2026-10-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A block the user writes under a put-back block (in its own file or in Holon) is deleted with it, with no disclosure, when the copy lets go and the deletion stands.
---

## Bug
Found by the D229 round-9 verifier (lane d229-move), probes VP-A (a grandchild typed under the put-back child in `DayPage.org`) and VP-B (the grandchild created in Holon). After `Overview.org` lets go of the child, the grandchild is gone from the store and from `DayPage.org`; the only condition left is the parent's `block-in-two-files`. Report: `lane-logs/d229r9v-verify.md`, Defect A.

## Root cause
`put_back_fingerprint` (`crates/holon-filesystem/src/file_sync_controller.rs`) hashed the put-back block alone. A new child does not change that hash, so the record still stood, and `apply_standing_deletions` / the ingest delete pass removed the block with `delete_in_tree`, which takes the whole subtree.

## Missing piece
The keystone's `DeleteLineFromFile` only deletes childless members, and the model never adds a child under an undone member; no org-suite test wrote under a put-back block.

## Remedy
Fixed in round 10: `put_back_fingerprint` hashes the block's subtree (less the descendants with an undone record of their own) and its parent, so a new child ends the deletion and keeps both blocks, with `DeletionEndedByEdit`; `end_edited_undone_deletions` runs to a fixpoint. The keystone model (`copies_model.rs` `first_edited_undone_member`) does the same. Org-suite `a_grandchild_written_under_the_put_back_child_stays_when_the_copy_lets_go` and `a_grandchild_created_in_holon_under_the_put_back_child_stays_when_the_copy_lets_go`; red `lane-logs/d229r10-red-vp.log`, green `lane-logs/d229r10-green-vp.log`, sabotage red `lane-logs/d229r10-sabotage.log`.
