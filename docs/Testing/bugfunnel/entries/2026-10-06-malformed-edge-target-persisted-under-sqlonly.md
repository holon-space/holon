---
id: 2026-10-06-malformed-edge-target-persisted-under-sqlonly
date: 2026-10-06
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  set_field("requires", ["a b"]) is refused under the Loro CRUD authority but
  accepted with Ok under SqlOnly, and the Loro
  org-reingest seam (BlockCellRegistry::write_field) stored the same target
  without a parse.
---

## Bug
Found by a verifier (lane D69.a round 7, `lane-logs/d69a-verify7.md`, defect
D2) by code reading. Executed in
`crates/holon/tests/malformed_reference_engine_route.rs`
(`a_malformed_edge_target_is_refused_by_name_under_both_authorities`): the
Loro provider refused the target by name, the SqlOnly engine's `set_field`
returned `Ok`. Red log `lane-logs/r8-red-d1d2.log`.

## Root cause
Three write paths took any string as an edge target:
- `SqlOperationProvider::set_field`, edge arm, and `partition_params`, edge
  arm, for create and update (`crates/holon/src/core/sql_operation_provider.rs`).
- `BlockCellRegistry::write_field`, edge arm
  (`crates/holon-loro/src/block_cell_registry.rs`), reached from
  `SqlBlockOperations::set_field`.

Nothing downstream catches the value: `block_requires.required_id` has no
foreign key by design (`crates/holon-turso/sql/schema/block_requires.sql`),
and `block_requirement_edges_matview` drops the row in its inner join without
a sign.

## Missing piece
No test writes a malformed edge target (COVERAGE). The test that pinned the
refusal drove only the Loro CRUD authority; the SqlOnly authority and the
reingest seam did not run in its wiring (ENVIRONMENT).

## Remedy
`EdgeField::refuse_malformed_targets` (`crates/holon-api/src/edge_field.rs`)
is the one parse of raw edge targets; it refuses an empty or malformed
reference target with an error naming the field and the value. The Loro
`set_field`, the SQL provider's `set_field` and `partition_params`, and
`BlockCellRegistry::write_field` call it before they write. Tests
`a_malformed_edge_target_is_refused_by_name_under_both_authorities` and
`block_cell_registry::tests::a_malformed_edge_target_is_refused_by_name`;
green logs `lane-logs/r8-green-d1d2.log`, `lane-logs/r8-green-loro.log`.
