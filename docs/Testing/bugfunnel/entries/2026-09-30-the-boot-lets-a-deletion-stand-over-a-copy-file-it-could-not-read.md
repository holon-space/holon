---
id: 2026-09-30-the-boot-lets-a-deletion-stand-over-a-copy-file-it-could-not-read
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  When the boot cannot read a copy file, the undone deletion is settled as if that file no longer held the block, and the deletion stands.
---

## Bug
Found by the D229 round-7 verifier (lane d229-move), probes VP-A/VP-A2. Report: `lane-logs/d229r7v-verify.md`, Defect 2.

## Root cause
`settle_undone_deletions` read holders only from files the scan had read successfully.

## Missing piece
The keystone has no transition that makes a vault file unreadable at boot.

## Remedy
Fixed in round 8: a copy file on disk that the scan did not read counts as a holder (R12.3); `an_undone_deletion_stays_while_its_copy_file_cannot_be_read`.
