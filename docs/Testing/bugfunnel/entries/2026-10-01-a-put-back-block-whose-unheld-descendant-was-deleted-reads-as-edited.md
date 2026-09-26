---
id: 2026-10-01-a-put-back-block-whose-unheld-descendant-was-deleted-reads-as-edited
date: 2026-10-01
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  When the user deletes a block together with a descendant that no copy holds, Holon puts the block back but at once says the user edited it, so the user's deletion never stands.
---

## Bug
Found by the D229 round-10 verifier (lane d229-move, Defect R10-1 in `lane-logs/d229r10v-verify.md`, probe VPX-B), a regression from round 9. DayPage.org holds `bulk-0-0 > kid > kid-gc`; Overview.org holds a copy `bulk-0-0 > kid` without `kid-gc`. The user deletes `kid` with `kid-gc` from DayPage.org. `kid-gc` is deleted, `kid` is put back. Right after that one save the conditions were `block-in-two-files` and `deletion-ended-by-edit` for `kid`, with no `deletion-undone-block-in-other-file`. When the user then deletes `kid` from Overview.org too, `kid` stays, and the disclosure still says it was edited.

## Root cause
Production. The put-back fingerprint (`put_back_fingerprint`, `crates/holon-filesystem/src/file_sync_controller.rs`) read the store BEFORE this ingest's delete ops were applied, so it included `kid-gc`. Every later `end_edited_undone_deletions` hashed the subtree without `kid-gc` and ended the record as an edit. Red: `lane-logs/d229r11-red.log` (left `["block:bulk-0-0-kid"]`).

## Missing piece
No invariant compares the put-back record with the store after the same save (ORACLE). The keystone `DeleteLineFromFile` deletes childless members only, and a copy is never stale against its owner's subtree, so the shape is not generated (COVERAGE).

## Remedy
- The covered set, fingerprint, disclosure, `persist_undone_deletions` and the crash point run after the ingest's ops are applied and flushed, still before any disk write (write-ahead kept).
- Org-suite tests `a_put_back_child_whose_grandchild_no_copy_holds_still_lets_its_deletion_stand` (red above; green `lane-logs/d229r11-green-vp.log`) and `an_edit_of_a_put_back_grandchild_ends_the_put_back_child_too` (sabotage with a single pass instead of the fixpoint reds it: `lane-logs/d229r11-sab-s2.log`).
- OPEN: the keystone generator does not yet delete a block with children from a file.
