---
id: 2026-09-29-an-undone-deletion-with-two-copy-files-never-stands
date: 2026-09-29
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  With two files holding a copy of a heading, a child the user deleted from its own file stays undone and disclosed forever, even after both copies let go of it.
---

## Bug
Found by the D229 round-5 verifier (lane d229-move), probe VP1 in `lane-logs/d229r5v-probes1.log`: `DayPage.org` owns `bulk-0-0` and its child; `Overview.org` and `Third.org` hold copies. The user deletes the child from `DayPage.org`, Holon puts it back; `Third.org` drops the whole heading, `Overview.org` drops the child. The child stays in the store and `deletion-undone-block-in-other-file` stays raised. Report: `lane-logs/d229r5v-verify.md`, Defect 1.

## Root cause
The undone deletion tracked one copy file per heading and ended only through that file's ingest; a copy file that dropped the whole heading was never removed from the holders (`file_sync_controller.rs`, undone-deletion tracking before round 6).

## Missing piece
The keystone model held one copy file per heading, so a second copy file was outside the alphabet.

## Remedy
Fixed in round 6: holders are tracked per undone block and every path on which a file stops holding it removes that file (design R10.1). The model holds a set of copy files per block (R10.2). Org-suite pins `an_undone_deletion_stands_when_the_other_copy_*`.
