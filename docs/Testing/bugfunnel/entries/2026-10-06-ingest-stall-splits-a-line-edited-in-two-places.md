---
id: 2026-10-06-ingest-stall-splits-a-line-edited-in-two-places
date: 2026-10-06
gap: COVERAGE
status: OPEN
summary: >-
  A line edited in the store and on disk while the first ingest's normalization write-back stalls becomes two blocks, and the user's line loses its id.
---

## Bug
Found by the fourth fresh-context verifier of lane d70d-write-back (ruling
D70.d), with a probe and a no-stall control (lane-logs/d70d-verify4.md, "D7";
lane-logs/v4-probe2.log, lane-logs/v4-probe3.log). The round-4 poll retry made
it worse; ruling D97.a removed the retry, and this residual stays.

Scenario: the first ingest's normalization write-back stalls (`StampChurn`).
The store edits `Buy milk` to `Buy oat milk`; the user edits the same line on
disk to `Buy soy milk`. After the poll the store holds both texts as two
blocks, the user's line under a new id, and the next block edit writes two
headlines. Without the stall, the line stays one block under its id
(control `a_line_edited_in_the_store_and_on_disk_after_the_first_ingest_stays_one_block`).

## Root cause
The stall arm tracks the file at the id-less bytes it parsed. The re-ingest
cannot bind the changed, id-less line to the stored block, so it mints a new
block and keeps the old one.

## Missing piece
COVERAGE: the stall tests stopped the churn and polled, but never combined a
stall with a user edit on disk or a pre-ingest. No keystone transition makes
a file's stamp churn, so the keystone cannot reach it.

## Remedy
OPEN. Pinned by the ignored test `a_line_edited_in_the_store_and_on_disk_during_an_ingest_stall_stays_one_block` in
crates/holon-orgmode/tests/writeback_compare_and_rename.rs (red for its
reason: lane-logs/d70d-r5-green.log, "ignored only" run). Fixed by the per-file
write-back state machine, Inc 2 (bound base) (plan `write-back-state-machine.md`,
ruling D97.a). Main b2a19917 has no stall (it overwrites a churning file), so
the scenario does not exist there.
