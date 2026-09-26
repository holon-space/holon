---
id: 2026-09-30-a-child-typed-back-while-holon-was-closed-is-deleted-by-the-standing-deletion
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A child the user typed back with new text, under the same id, while Holon was closed, is deleted at the next boot by the undone deletion that ended while Holon was closed.
---

## Bug
Found by the D229 round-8 verifier (lane d229-move), probe `vp3_a_reauthored_child_meets_the_standing_deletion` in `lane-logs/d229r8v-probes.log`: the new text is gone from `DayPage.org` and from the store, and no condition names it. Report: `lane-logs/d229r8v-verify.md`, Defect 1.

## Root cause
The undone-deletion record carried no content, so the boot settle deleted the block as the store held it after the scan, whatever the user had written.

## Missing piece
The keystone does not edit an owner file while Holon is closed; no model state describes a re-authored put-back.

## Remedy
Fixed in round 9: the record keeps a fingerprint of the put-back version, and a block that differs from it ends the deletion and is kept, with `DeletionEndedByEdit` (R13.1). Org-suite `a_child_typed_back_with_new_text_while_closed_keeps_the_new_text` and `a_put_back_child_edited_in_holon_stays_when_the_copy_lets_go` (red: `lane-logs/d229r9-red.log`); keystone records `deletion-undone-by-a-copy-ends-when-holon-edits-the-child-*`.
