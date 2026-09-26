---
id: 2026-09-30-deleting-an-unread-copy-file-leaves-the-deletion-undone-until-restart
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  When the user deletes a copy file the boot could not read, the undone deletion does not stand and its disclosure names the vanished file until the next restart.
---

## Bug
Found by the D229 round-8 verifier (lane d229-move), probes VP-2 and VP-4 in `lane-logs/d229r8v-probes{,2}.log`. Report: `lane-logs/d229r8v-verify.md`, Defect 2.

## Root cause
The file-deleted path removed holders only through `self.copies`, which a refused boot read never filled.

## Missing piece
The keystone has no unreadable-file-at-boot state.

## Remedy
Fixed in round 9: `forget_file_state` removes the file from every undone deletion's holders (R13.2); `an_unread_copy_file_the_user_deletes_lets_the_deletion_stand` (red: `lane-logs/d229r9-red.log`).
