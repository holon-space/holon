---
id: 2026-10-01-a-restore-after-a-crash-between-write-back-and-hash-record-is-skipped-at-boot
date: 2026-10-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  When Holon died after a write-back reached disk and before it recorded the written bytes' hash, a restore of the bytes the row still named was skipped at the next boot and later rendered over.
---

## Bug
Found by the D229 round-11 verifier by reading the code (O1 in `lane-logs/d229r11v-verify.md`), lane d229-move. Round 11 wrote the hash of the written bytes to the `file` row AFTER `fs.write` in `write_back_or_skip_readonly` (`crates/holon-filesystem/src/file_sync_controller.rs`). A crash between the two left the row on the previous hash. If the user then restored exactly those earlier bytes while Holon was closed, the boot's fast path matched the stale hash and skipped the file.

## Root cause
The disk write and the record of what was written were two steps with the record second. The undone-deletion record already used write-ahead; the `file` row did not.

## Missing piece
No transition or test kills the controller between a write-back and its record. The keystone has no crash transition at that point, so the window could only be found by reading.

## Remedy
D18.b (round 12): the write-ahead writes the empty hash to the row BEFORE the disk write (once per file until the next stamp), so any crash leaves a row that makes the next boot read the file. The real hash is written at idle and at a clean stop. New crash point `after_write_back` (feature `crash-injection`). Org-suite test `a_restore_after_a_crash_before_the_written_hash_is_recorded_is_read_at_boot`: red on the round-11 controller (`lane-logs/d229r12-red-r11tree.log`, "the boot did not read the restore"), green (`lane-logs/d229r12b-green2.log`), red again with the write-ahead moved after the write (sabotage S-O1, `lane-logs/d229r12b-teeth-S-O1.log`). Cost of a crash: the next boot reads every file written since the last stamp; measured in the test profile on a 20-file vault with all 20 written, that is one cold ingest (`lane-logs/d229r12c-boot-crash-dv20.log`).
