---
id: 2026-10-01-a-restore-of-pre-paste-bytes-into-a-copy-holding-file-is-unpinned
date: 2026-10-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  No test failed when a file that holds a copy wrote nothing to its `file` row, although then a restore of the bytes from before the paste is skipped at boot and the store keeps Holon's edit.
---

## Bug
Found by the D229 round-11 verifier by sabotage (G2 in `lane-logs/d229r11v-verify.md`), lane d229-move. Sabotage S3b (an early `return Ok(())` for a copy holder in `persist_disk_hash_for`, which is the round-10 behaviour) kept the full org_suite green (109/109). Probe VPR-7 showed the harm: read Notes.org, paste a copy into it, edit a Notes.org block in Holon, close, restore the pre-paste bytes; under S3b the boot skipped the restore and the store kept "Note edited in Holon" (`lane-logs/d229r11v-sab-s3b-vpr7.log`).

## Root cause
A copy holder's row must hold the empty hash so that every boot reads the file. If its row is left alone, it keeps the hash of an earlier ingest, and a restore of exactly those bytes matches it.

## Missing piece
VPR-7 was a probe, not a committed test.

## Remedy
Org-suite test `a_restore_of_pre_paste_bytes_while_closed_is_read_at_boot` (VPR-7 as a test, const `NOTES_WITH_A_COPY`). Green on round 11 and on D18.b (`lane-logs/d229r12b-green2.log`); red under S3b on the D18.b controller (`lane-logs/d229r12b-teeth-S3b.log`). D18.b keeps a copy holder's row on the empty hash; the idle and stop stamps skip copy holders.
