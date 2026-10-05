---
id: 2026-10-06-ingest-stall-undoes-a-line-deleted-on-disk
date: 2026-10-06
gap: COVERAGE
status: OPEN
summary: >-
  A line the user deletes from a vault org file while the first ingest's normalization write-back stalls stays in the store, and the next write-back of the file puts it back on disk.
---

## Bug
Found by the fourth fresh-context verifier of lane d70d-write-back (ruling
D70.d), with a probe and a no-stall control (lane-logs/d70d-verify4.md, "D6";
lane-logs/v4-probe2.log, lane-logs/v4-probe3.log). The round-4 poll retry made
it worse; ruling D97.a removed the retry, and this residual stays.

Scenario: a two-headline file without `:ID:` drawers is ingested while
another process keeps rewriting it with the same bytes, so the normalization
write-back stalls (`StampChurn`). The user deletes `* Call mom` from the file.
The poll re-ingests the file, but `Call mom` stays in the store. The next
block edit renders the store and writes `Call mom` back to disk. Without the
stall, the deletion reaches the store (control
`a_line_deleted_on_disk_after_the_first_ingest_stays_deleted`).

## Root cause
The stall arm tracks the file at the bytes it parsed, and those bytes carry
no `:ID:`. The re-ingest diffs the user's file against that id-less base, so
it cannot bind the removed headline to the stored block and applies no
delete.

## Missing piece
COVERAGE: the stall tests stopped the churn and polled, but never combined a
stall with a user edit on disk or a pre-ingest. No keystone transition makes
a file's stamp churn, so the keystone cannot reach it.

## Remedy
OPEN. Pinned by the ignored test `a_line_deleted_on_disk_during_an_ingest_stall_stays_deleted` in
crates/holon-orgmode/tests/writeback_compare_and_rename.rs (red for its
reason: lane-logs/d70d-r5-green.log, "ignored only" run). Fixed by the per-file
write-back state machine, Inc 2 (bound base) (plan `write-back-state-machine.md`,
ruling D97.a). Main b2a19917 has no stall (it overwrites a churning file), so
the scenario does not exist there.
