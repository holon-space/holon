---
id: 2026-11-02-block-history-floor-counts-creates-of-other-databases
date: 2026-11-02
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  After a boot while another process holds the database file, the keystone
  op_group floor expected the `block_history` rows of creates that the
  in-memory database never held, so `inv-history-records-all-creates` was red
  with no product defect behind it.
---

## Bug

The keystone tour discovery run stopped at `RebootWhileDatabaseHeld` with
`MISSED HISTORY: block_history has 0 distinct op_group(s) but the oracle drove
10 UI create(s)`. The tour then listed `RebootWhileDatabaseHeld` and
`ReleaseDatabaseAndRestart` as not toured. Found by an agent (keystone tour
discovery), card holon-work#811.

## Root cause

`block_history` lives only in the database file, like navigation history and
pins. A boot while the file is held runs on an empty in-memory database
(Loro projects the blocks, but history is not in Loro), and the restart after
the release opens the file, which does not hold the in-memory session's
creates. The oracle floor in
`crates/holon-integration-tests/src/pbt/composed/harness.rs` (`catalog_report`)
counted every UI create since the first boot.

Ruling (Martin, 2026-11-02): "Test-model fix only: block_history is DB-file-only
state like nav history and pins; the floor counts per DB epoch".

## Missing piece

The reference model stashed the database file's navigation and pins across a
held boot (`DatabaseUiRows` in `ui_actor_state.rs`), but the history floor had
no notion of which database the SUT runs on.

## Remedy

The floor subtracts the UI creates the current database does not hold
(`UnheldHistory` in `harness.rs`, updated at every boot). Locked by the
hand-authored case `block-history-floor-counts-per-database-epoch` in
`hand-authored-regressions/keystone.jsonl`. Both held-database kinds are now
keystone tour steps.

Open, as a product card (not this fix): history that survives a held session
(kept in Loro, or merged into the file on release) — holon-work#811 until main
cuts the separate card.
