---
id: 2026-10-12-undo-of-a-typed-entity-op-reads-block-raw
date: 2026-10-12
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Undo of a create, set_field or delete on a free-standing typed entity failed
  or was refused as stale, because the undo precondition reader looked up the
  typed id in `block_raw` instead of the type's own table.
---

## Bug
Found by the composed keystone in lane `loro-tes-2` (holon-space/holon-work#168),
after the oracle started to model typed entity ops as undoable (ruling Q2:
history covers every entity op). Two signatures, both at
`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:4353`:

- `undo failed: Some(undo reader query: ... no such column: role)` — forced
  full draw `storage={Loro, Org, Markdown, Turso}`, red log
  `target/ltes2-undo-red.log`.
- `undo returned non-applied (nothing to undo or stale)` — draw
  `storage={Org, Turso}`, after two `CreateTypedEntity`, log
  `target/ltes2-ks2-any.log`.

The SQL-only draw fails too, so the defect is older than the Loro store.

## Root cause
Every typed op returns an inverse with column deltas, so it lands on the undo
stack. `SqlUndoStateReader` (`crates/holon/src/api/undo_persistence.rs`) read
every precondition from the one table it was built with, `BLOCK_WRITE_TABLE`
(`crates/holon/src/api/backend_engine.rs`). For a typed id such as
`person:person-0` the query either named a column `block_raw` does not have
(error) or found no row (stale, so the undo was refused).

## Missing piece
The oracle kept only block snapshots on its undo/redo stacks and typed
transitions pushed no history entry, so no invariant compared a typed undo with
the SUT.

## Remedy
The reader routes by URI scheme: `block` reads the block write table, any other
scheme reads the raw table of the type the `TypeRegistry` declares for it, and
an undeclared scheme is an error. The oracle's history entries hold the typed
rows too, and the typed transitions push an undo snapshot.
