---
id: 2026-10-06-pre-ingest-stall-loses-a-store-edit
date: 2026-10-06
gap: COVERAGE
status: OPEN
summary: >-
  A store edit whose block-driven write-back first re-ingests a changed file
  is reverted in the store by that pre-ingest; when the writes then stall, the
  edit is on neither disk nor store and lives only in the holder.
---

## Bug
Found by the fourth fresh-context verifier of lane d70d-write-back (ruling
D70.d), with a probe and a no-stall control (lane-logs/d70d-verify4.md, "D8";
lane-logs/v4-probe2.log, lane-logs/v4-probe3.log). The round-4 poll retry made
it worse; ruling D97.a removed the retry, and this residual stays.

Scenario: the store edits `Buy milk` to `Buy oat milk`; the file changed on
disk (its document id was removed), so the block-driven write-back first
re-ingests it; that pre-ingest's normalization write and the block write
both stall (`StampChurn`). The pre-ingest takes the file's `Buy milk` over
the store edit. The stall stays disclosed (the refused write-back stands
until a write lands), and the next block edit writes `Buy oat milk` to
disk, but the store keeps `Buy milk`.

The store loss is not stall-specific. The A/B probe (lane-logs/d70d-r5-ab-main.log
on main b2a19917, lane-logs/d70d-r5-ab-reduced.log on the reduced tree, probe
`D8-nostall`) shows the store at `Buy milk` after the pre-ingest on both trees
without a stall, while the block write puts `Buy oat milk` on disk; the poll
does not re-ingest it (disk equals the tracked projection), so store and disk
disagree. On main the block write also misses the disk in the same call
(the D86.a defect); the reduced tree writes it. The stall removes the disk copy
as well.

## Root cause
Not measured. Observation: the pre-ingest of the changed file leaves the
store at the file's text for a block the file did not change. Candidates:
the ingest applies every block of a changed file as an update, or its diff
base predates the store edit.

## Missing piece
COVERAGE: the stall tests stopped the churn and polled, but never combined a
stall with a user edit on disk or a pre-ingest. No keystone transition makes
a file's stamp churn, so the keystone cannot reach it.

## Remedy
OPEN. Pinned by the ignored test `a_block_edit_whose_pre_ingest_stalls_stays_in_the_store` in
crates/holon-orgmode/tests/writeback_compare_and_rename.rs (red for its
reason: lane-logs/d70d-r5-green.log, "ignored only" run). No increment of the per-file write-back state machine plan
(`write-back-state-machine.md`, ruling D97.a) covers the store reversion yet;
its Inc 4 covers the disclosure part. Main b2a19917 has no stall, but has the
store reversion.
