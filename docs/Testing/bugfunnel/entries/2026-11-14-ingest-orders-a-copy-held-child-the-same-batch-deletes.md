---
id: 2026-11-14-ingest-orders-a-copy-held-child-the-same-batch-deletes
date: 2026-11-14
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  An org ingest under a copied heading failed partway and quarantined the
  file, because the sibling order it wrote still held a child that the same
  ingest batch had deleted.
---

## Bug
A random keystone run in the integration gate went red on
`inv-no-observed-errors`: `ordering.place_all failed` for a journal file, which
then stayed quarantined from write-back. The same file re-failed at every file
event. Only some seeds reach it, so the keystone smoke step turned red at
random.

## Root cause
Two independent defects met:
- The ingest keeps, under a copied heading, the store siblings that a copy
  file holds (`kept_order`, crates/holon-filesystem/src/file_sync_controller.rs).
  A child added under the heading after the paste is not held by the copy, so
  the delete pass of the same batch deletes it when its line leaves the owner
  file. `kept_order` still listed it, and the place step met a child with no
  row.
- `place_all` (crates/holon/src/core/sql_block_operations.rs) gave an id
  without a cache row a default key and wrote row by row, so it failed at the
  first missing row after earlier keys were already written.

Evidence: the origin probe tagged the failing id
`base=true in_store=true held=true kept=true created=false parse=false`
(deleted from the file, still in `kept_order`).

A second path reached the same failure: at boot with a boot hash row, the
classifier skipped a block the file holds but the store lost, yet kept it in
the order.

## Missing piece
- No case reached "child added under a copied heading after the paste, then
  deleted from its owner file" except a rare random sequence of
  PasteBlockCopy, CreateBlockUnderFocus and DeleteLineFromFile.
- Litmus COVERAGE: the catalog can generate the sequence, but almost never
  does; no focused generator or static example covered it.
- Litmus ORACLE: `inv-no-observed-errors` went red whenever a case hit the
  state, so the invariant was sufficient.

## Remedy
- `place_all` reads `block_raw` once for all ids, refuses with one error that
  names the parent and every missing id, and writes the order in one
  transaction.
- The ingest drops from the kept order every id the batch deletes, and
  re-creates from the file a block the store lost.
- Pinned by `ingest_under_a_copied_heading_orders_only_children_with_a_row`
  (crates/holon-integration-tests/tests/org_suite/copied_heading_children_keep_their_rows.rs),
  `place_all_refuses_ids_without_a_row_and_writes_nothing`
  (crates/holon-integration-tests/tests/place_all_restates_total_sibling_order.rs),
  and the keystone static example
  `a-child-added-under-a-copied-heading-and-deleted-from-its-owner-leaves-the-sibling-order`.
