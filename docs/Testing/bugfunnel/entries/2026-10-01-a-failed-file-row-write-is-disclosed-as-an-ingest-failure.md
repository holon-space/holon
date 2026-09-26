---
id: 2026-10-01-a-failed-file-row-write-is-disclosed-as-an-ingest-failure
date: 2026-10-01
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  When the `file` row could not be written during a write-back, the user saw a `vault-ingest-failed` condition on the file, which named an ingest failure instead of the store-side cause, and disk sync of that file stopped.
---

## Bug
Found by the D229 round-11 verifier by probe VPR-5 (O2 in `lane-logs/d229r11v-verify.md`, `lane-logs/d229r11v-probes-vpr56.log`), lane d229-move. With the `file` table dropped mid-session, Holon edit 1 reached disk, its hash write failed, the echo of that write was ingested, the ingest failed too and quarantined Notes.org with `vault-ingest-failed`. Edit 2 stayed in the store only. The failure was visible, but its label pointed at the wrong layer.

## Root cause
The row write came after the disk write (`write_back_or_skip_readonly`, `crates/holon-filesystem/src/file_sync_controller.rs`), so the disk write produced an echo, and the first failure the user saw was the echo's ingest, not the write-back's row write.

## Missing piece
No assertion checked which cause a disclosure names. Tests checked that something was disclosed, not that it named the failing step.

## Remedy
D18.b (round 12): the row write happens before the disk write; when it fails, nothing is written, so there is no echo. The disclosure is `writeback-degraded` on the file, and its cause names the file row. A failed stamp at idle or stop raises `written-files-unrecorded` on the vault, naming the files. Pinned by `a_write_back_the_file_table_cannot_record_is_not_written_and_says_why` (checks the condition kind and the "file row" cause) and `written_bytes_the_file_table_cannot_record_are_disclosed` (green: `lane-logs/d229r12b-green2.log`).
