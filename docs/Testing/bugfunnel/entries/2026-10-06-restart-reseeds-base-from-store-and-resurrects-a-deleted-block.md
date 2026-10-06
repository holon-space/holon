---
id: 2026-10-06-restart-reseeds-base-from-store-and-resurrects-a-deleted-block
date: 2026-10-06
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  When the diff base is seeded from the store (empty last_projection, e.g. after
  a restart), a block Holon deleted is absent from the base, the classification
  skips it without asking ever_seen, and a file that still holds the block
  re-creates it.
---

## Bug
Found by a verifier (lane D69.a round 6, claim 6) by reading the code; the
verifier could not stage it. Round 7 stages it with a restart.

## Root cause
`ingest_file` seeds the base from `block_reader.get_blocks` when
`last_projection` is empty (`crates/holon-filesystem/src/file_sync_controller.rs`,
"(Re)seed the base"). A deleted block is not in that seed. The classification
arm for a file block with no `old_blocks` entry and no deleted parent is
`continue`, so `ever_seen` is never asked and the create/update pass re-creates
the block. The arm is not new in round 6; the write-back state machine plan owns
it (`~/.claude/plans/write-back-state-machine.md`, section 11 A11, Inc 2).

## Missing piece
No keystone transition restarts the app between a Holon delete and an editor
save of a stale copy. Pinned by the ignored harness test
`a_restart_then_a_file_still_holding_a_holon_deleted_block_keeps_it_deleted`
(`crates/holon-integration-tests/tests/loro_suite/loro_delete_races_file_edit.rs`).

## Remedy
OPEN. Not fixed in D69.a. D97 write-back state machine Inc 2 (A11) classifies
per block against the bound base; un-ignore the test when it lands.
