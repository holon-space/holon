---
id: 2026-07-21-boot-panic-unmasked-directory-fix-which
date: 2026-07-21
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Boot PANIC ~1.1s in (UNMASKED by the Directory fix, which let boot reach
  it): `MutableTree::update on unknown node` (mutable_tree.rs:294).
  `MutableTree::remove` evicts the node AND its whole subtree but returns
  `()`, so the tree driver (reactive_view.rs RemoveAt/Pop) drops ONE key from
  `row_map`/`key_index` while upstream `ReactiveRowSet` keeps live cells for
  every surviving descendant; the next CDC touch of a survivor yields UpdateAt
  for a node the tree no longer has. Boot trigger: `retain_keys` on a
  generation bump drops a stale PARENT while its children are retained.
source_line: 1071
---

## Bug

Boot PANIC ~1.1s in (UNMASKED by the Directory fix, which let boot reach
it): `MutableTree::update on unknown node` (mutable_tree.rs:294).
`MutableTree::remove` evicts the node AND its whole subtree but returns
`()`, so the tree driver (reactive_view.rs RemoveAt/Pop) drops ONE key from
`row_map`/`key_index` while upstream `ReactiveRowSet` keeps live cells for
every surviving descendant; the next CDC touch of a survivor yields UpdateAt
for a node the tree no longer has. Boot trigger: `retain_keys` on a
generation bump drops a stale PARENT while its children are retained.

## Missing piece

keystone has ZERO references to MutableTree/reactive_view/ReactiveViewModel
— the whole ReactiveRowSet->keyed_signal_vec->MutableTree chain sits above
its observation boundary; secondary: all ~17 mutable_tree tests drive the
tree DIRECTLY, none exercises the driver+tree pair where the contract breaks

## Remedy

FIXED (remove() returns evicted descendants, mirroring insert()'s
adopted-ids pattern; driver reinstates survivors as stranded roots —
required, since key_index is upstream-position-indexed and dropping
survivors would misalign later index-based diffs and remove the WRONG key;
remove() on unknown id now asserts too; red-first driver-level test;
holon-frontend 362/362)
