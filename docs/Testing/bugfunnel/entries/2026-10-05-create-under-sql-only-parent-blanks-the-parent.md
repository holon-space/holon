---
id: 2026-10-05-create-under-sql-only-parent-blanks-the-parent
date: 2026-10-05
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  In a Loro vault, a create under a parent that only the SQL projection held
  stood up an empty placeholder root for that parent in Loro, and its
  projection wrote "" over the parent's SQL row.
---

## Bug

Found by agent exploration (code reading) on the D69.a lane, then measured.
`BlockOrdering::create_in_tree` under a parent with a SQL row and no Loro node
returned `Ok(true)`. After quiescence the parent's `block_raw.content` was ""
(it was "stranded parent"). Red log:
`lane-logs/loro-unheld-red4.log` in the d69a-all-writes workspace.

## Root cause

`BlockCellRegistry::create_entity` resolves the parent through
`resolve_parent_or_placeholder` (crates/holon-loro/src/block_cell_registry.rs).
A parent the tree does not hold gets an EMPTY placeholder root. That is correct
for a forward reference (a child reached before its parent's create, parent in
neither store). For a parent the SQL projection already holds, the placeholder
is a write on a block the authority does not hold, and its outbound projection
overwrites the SQL row with "".

## Missing piece

The keystone never reaches a state where SQL holds a block that Loro does not
(an unseeded vault), so no generated create can land under such a parent.

## Remedy

`SqlBlockOperations::refuse_create_under_projection_only`
(crates/holon/src/core/sql_block_operations.rs) refuses the create with
`BlockNotInWriteAuthority` naming the PARENT, on `create_in_tree` and
`create_in_tree_batch` (parents created in the same batch are skipped). The
`BlockOrdering` DI instance now carries the write authority
(crates/holon-loro-wiring/src/event_infra_module.rs). A parent no store holds
still gets its placeholder. Pinned by
`loro_writes_on_unheld_blocks::a_create_under_a_parent_only_the_projection_holds_is_refused`
(loro_suite).
