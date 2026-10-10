---
id: 2026-10-10-block-tree-rebuild-drops-sqlonly-rows
date: 2026-10-10
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  A stored block-tree table whose shape changed was dropped and refilled from
  the org files and the Loro store; with the CRDT layer off it is the only
  durable copy, so every block the org files do not hold was lost at boot.
---

## Bug

Lane `block-l1` made `block_raw.block_type` nullable and deleted
`completed`. On a database with the older `block_raw`, boot found a
difference no column addition fixes, and `rebuild_table`
(`crates/holon-turso/src/table_shape.rs`) dropped the table and created it
empty. A drifted junction table emptied `block_raw` too (`emptied_with`).
With `crdt.enabled = false` (SqlOnly) `block_raw` and its junctions are the
only durable copy of the block tree (`docs/Architecture/Model.md`, storage
axis; ADR 0036 D1), so the rows were gone.

Found by the verifier of lane `block-l1` (`lane-logs/bl1-verify3.md`, D1/D2)
through `column_added_under_dependent_matview.rs` and the SqlOnly SQL-row
test. Red reproduction: `lane-logs/bl1r4-red-loro-KEEP.log`
(`every_legacy_row_survives_the_reshape_without_the_loro_store`).

## Root cause

The table classes held `block_raw` as refilled by the org files and the Loro
store in every wiring. In SqlOnly no store refills it.

## Missing piece

ENVIRONMENT: every legacy-shape test booted the Loro wiring, where the
refill hides the drop. COVERAGE: no test booted SqlOnly over a drifted
`block_raw`.

## Remedy

A drifted block-tree table (`table_classes::carries_rows`) keeps its rows:
`reshape_table` copies them into the declared shape in one transaction and
raises `table-reshaped`; a row the declaration cannot hold fails the schema
module and leaves every table as it was. Tests:
`crates/holon-turso/tests/rebuilt_table_drift.rs`,
`crates/holon-integration-tests/tests/loro_suite/loro_block_type_on_a_legacy_schema.rs`.

Open: `db_open` still deletes a whole database it cannot use, which in
SqlOnly loses the block tree the same way.
