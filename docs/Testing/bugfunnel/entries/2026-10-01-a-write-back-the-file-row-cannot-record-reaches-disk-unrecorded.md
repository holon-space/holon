---
id: 2026-10-01-a-write-back-the-file-row-cannot-record-reaches-disk-unrecorded
date: 2026-10-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  When the `file` table refused a write, Holon still wrote its edit to the org file, so the disk held bytes no row described, and no test failed when the failure was ignored.
---

## Bug
Found by the D229 round-11 verifier by sabotage (G1 in `lane-logs/d229r11v-verify.md`), lane d229-move. `let _ =` on the write-back's hash write kept every test green (`lane-logs/d229r11v-sab-s4b.log`, `-s4b-hash.log`). In round 11 the hash write came after the disk write, so a refused row write still left the edit on disk with no record.

## Root cause
`write_back_or_skip_readonly` (`crates/holon-filesystem/src/file_sync_controller.rs`) wrote the file first and the row second. The error of the row write was returned, but only after the disk already held the new bytes.

## Missing piece
No test made the `file` table refuse a write during a write-back. Fail-loud on this path had no teeth.

## Remedy
D18.b (round 12): the row write comes first. If it fails, the file is not written, the edit is kept in `refused_writebacks`, and the file carries a `writeback-degraded` condition whose cause names the file row. Org-suite test `a_write_back_the_file_table_cannot_record_is_not_written_and_says_why` (drops the `file` table, then edits): red on the round-11 controller (`lane-logs/d229r12-red-r11tree.log`, "the edit reached disk with no record of the bytes written"), green (`lane-logs/d229r12b-green2.log`), red with `let _ =` on the write-ahead (sabotage S-G1, `lane-logs/d229r12b-teeth-S-G1.log`).
