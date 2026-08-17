---
id: 2026-07-17-missed-history-loro-consolidator-wiring-records
date: 2026-07-17
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  C2 MISSED HISTORY in Loro-consolidator wiring: `block_history` records ZERO
  op_groups for creates that route through the Loro backend (default
  `crdt.enabled=true` app wiring) — `inv-history-records-all-creates` op-group
  floor missed, surfaced on `CreateBlockUnderFocus` (and every
  create/split/page mint) once the sibling display-placement/advice WIP reds
  are lifted
source_line: 816
---

## Bug

C2 MISSED HISTORY in Loro-consolidator wiring: `block_history` records ZERO
op_groups for creates that route through the Loro backend (default
`crdt.enabled=true` app wiring) — `inv-history-records-all-creates` op-group
floor missed, surfaced on `CreateBlockUnderFocus` (and every
create/split/page mint) once the sibling display-placement/advice WIP reds
are lifted

## Missing piece

keystone runs the history correspondence in ALL full-mode configs but its
RED is masked: proptest shrinks each failing case onto whichever sibling WIP
red (`inv-display-placement-canonical-inert`, `inv-advice-rows-woven`) fails
first, and shrinking removes the create transitions the history floor needs
— so the history red never reaches the minimal case

## Remedy

ROOT-CAUSED + FIXED (2026-07-17): the engine's history chokepoint
(`operation_engine.rs:903` `record_history`) builds one `HistoryEvent` per
`OperationResult.changes` field delta; `record_batch([])` early-returns.
`SqlOperationProvider::create` reports the create as a `FieldDelta(id, "id",
Null, id)` (sql_operation_provider.rs:1710) → 1 op_group recorded. But
`LoroBlockOperations::create` returned `OperationResult::new(vec![],
delete-inverse)` (EMPTY changes, loro_block_operations.rs:633) →
record_batch no-op → a Loro-backed create records NO history. Since
block_history is a Turso table present in every full-mode config (SutHistory
cap unconditionally wired) while the content-write may be Loro, the floor
(counts synthetic→real mints across ALL wirings) exceeds the SUT's 0
op_groups. FIX: Loro create now emits the same `FieldDelta(id, "id", Null,
minted-id)` as the SQL path (non-vacuous → undo journaling unchanged;
upsert-over-existing stays irreversible/no-delta). Regression test
`create_reports_id_field_delta_for_history` (loro_block_operations.rs).
Reproduced via `HOLON_PBT_DISPLAY_PLACED=1 PROPTEST_CASES=48` keystone
(Loro/Loro+Turso wirings)
