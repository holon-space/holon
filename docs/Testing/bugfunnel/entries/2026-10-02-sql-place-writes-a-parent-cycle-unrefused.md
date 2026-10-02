---
id: 2026-10-02-sql-place-writes-a-parent-cycle-unrefused
date: 2026-10-02
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  The SQL placement path writes a block under its own descendant without a cycle check,
  so in SqlOnly mode (and in Loro mode for a block with no Loro node) a dispatched
  `move_block` can store a parent cycle that Loro mode refuses.
---

## Bug

A read-only architecture study (2026-10-02, D26.b risk pass on tree depth and cycles)
found that the SQL write path for a block's position has no parent-cycle guard. Nothing
ran; this comes from reading the code.

## Root cause

- `SqlBlockOperations::place` (`crates/holon/src/core/sql_block_operations.rs:494-541`)
  first tries `write_position`. When that returns `false`, it mints a key and calls
  `place_row` (`:537-539`). It has an idempotency check (`:514-530`) but no cycle check.
- `SqlOperationProvider::place_row` (`crates/holon/src/core/sql_operation_provider.rs:868-902`)
  writes `parent_id` and `sort_key` in one transaction. The only check is the FK, and the FK
  proves only that the parent exists.
- `write_position` returns `false` in SqlOnly mode. In Loro mode it also returns `false` for
  every block with no Loro node (`crates/holon-loro/src/block_cell_registry.rs:452-461`).
- The code itself says the state is reachable: `descendant_ids`
  (`sql_operation_provider.rs:1249-1257`) refuses a stored cycle because "`move_block` has no
  reparent-under-own-descendant guard and is dispatchable".
- Loro mode refuses the move twice: `BlockMutation::validate`
  (`crates/holon-api/src/block_mutation.rs:121-126`, `WouldCreateCycle`) runs at
  `crates/holon-loro/src/loro_backend.rs:5306-5322`, and `tree.mov` checks again at `:5330`.
  The SQL paths never call `BlockMutation::validate`.
- Readers detect a stored cycle only after the fact (`descendant_ids`, the purge cascade at
  `sql_operation_provider.rs:1950-1955`). The Turso IVM recursive views have no guard.

## Missing piece

The keystone runs SqlOnly cases, but it never generates the move that makes the cycle. The
`DragDropBlock` precondition rejects a target that is a descendant of the source
(`crates/holon-integration-tests/src/pbt/transitions/drag_drop_block.rs:172-185`,
`Reason::CyclicParentMove`). No other transition dispatches `move_block` with a descendant as
the parent. So the refusal is never tested, and `no_parent_cycles` passes without being
exercised.

## Remedy

OPEN. Rung that closes the gap: a keystone transition (or a relaxed `DragDropBlock` variant)
that dispatches `move_block(source, parent = a descendant of source)`. The reference model
expects a refusal and an unchanged tree. Run it in both SqlOnly and Loro mode. It must go red
in SqlOnly, on `no_parent_cycles` or on the ref/SUT tree comparison, before the fix. Fix
direction from the study: call `BlockMutation::validate` at the one write gate (the dispatcher
admission step, D6.d), so the SQL paths and `update_parent_id` share the Loro check.
