---
id: 2026-10-01-no-test-reads-the-hash-recorded-at-a-clean-stop
date: 2026-10-01
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  No test fails when the stop stamp writes nothing; the only effect is that the next boot reads every file Holon wrote since the last idle stamp again.
---

## Bug
Found by the D229 round-12 implementers (lane d229-move) while pinning D18.b. The clean stop records the hashes of the files Holon wrote (`stamp_written_hashes` in the loop's `shutdown.cancelled()` arm, `crates/holon-orgmode/src/di.rs`). If it wrote nothing, the rows would keep the empty hash, which is safe: the next boot reads those files. So no correctness test can go red.

## Root cause
A missing stop stamp costs boot time only, and the `file` row cannot be read after `stop_app` (the DB is closed), so a test cannot read the row directly.

## Missing piece
An assertion on boot work after a clean stop: for example, a count of files the boot reads, or a `file`-row read in the next session before its ingest runs.

## Remedy
Open. Indirect measurement only (test profile, 20-file vault, `lane-logs/d229r12c-boot-clean-dv20.log`): stop 300 ms after the last write-back (before any idle stamp can run), and boot2 takes the fast path; a crash at the same point makes boot2 read every written file (`lane-logs/d229r12c-boot-crash-dv20.log`).
