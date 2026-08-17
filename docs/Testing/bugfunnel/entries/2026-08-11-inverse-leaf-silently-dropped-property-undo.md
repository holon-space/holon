---
id: 2026-08-11-inverse-leaf-silently-dropped-property-undo
date: 2026-08-11
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  The inverse of a leaf `delete` silently dropped every property, so undo
  resurrected the block stripped of them
source_line: 730
---

## Bug

(increment B2 / task #22 "inverses carry explicit NULLs"; found by an AGENT
probing verifier finding F3 — why the `partition_params` properties change
was unpinned; no automated test produced it) **The inverse of a leaf
`delete` silently dropped every property, so undo resurrected the block
stripped of them** — a `TODO` block returned with no `task_state`.
`capture_row` returns the `properties` column DECODED (measured:
`Some(Object({"keeper": String("survives")}))`), but `partition_params`
matched only the JSON-string shape via `value.as_string()`, so the Object
landed in no bucket and no `properties` column reached the resurrecting
INSERT. Pre-existing; removing the `capture_row` NULL filter is what made it
visible.

## Root cause

increment B2 / task #22 "inverses carry explicit NULLs", found by an AGENT
probing why the `partition_params` properties change was unpinned (verifier
finding F3) — no automated test produced it: **the inverse of a leaf
`delete` silently dropped every property, so undo resurrected the block
stripped of them** (a `TODO` block came back with no `task_state`).
`capture_row` hands the `properties` column back DECODED — measured
`Some(Object({"keeper": String("survives")}))`, not the JSON string a live
caller passes — and `partition_params` matched only `value.as_string()`, so
the Object fell into no bucket, no `properties` column reached the
resurrecting INSERT, and the loss was silent. COVERAGE primary: the
triggering interaction is not generatable — the catalog has NO
leaf-block-delete transition (`delete_backward` is backspace/join,
`delete_document` deletes documents), so no sequence reaches "delete a block
carrying properties, then undo". ORACLE secondary: had it been generated,
only `task_state` would have convicted it (`task_state_storage_coherence`);
no invariant compares the general `properties` map, so an arbitrary property
would have vanished unobserved. Missing piece: a leaf block-delete
transition, plus a properties-equality clause in the block/ref comparison.
Fixed by re-serializing an Object into the existing merge path in
`partition_params`; pinned at the provider contract by
`delete_inverse_classification_tests::a_delete_inverse_resurrects_the_rows_properties`,
reversion-probe red logged (`left: None / right: Some("survives")`).)

## Missing piece

No leaf-block-delete transition exists in the catalog (`delete_backward` is
backspace/join, `delete_document` deletes documents), so "delete a block
carrying properties, then undo" is ungeneratable; and no invariant compares
the general `properties` map, so only `task_state` would have convicted it.

## Remedy

FIXED — Object re-serialized into the existing merge path in
`partition_params`; pinned by
`delete_inverse_classification_tests::a_delete_inverse_resurrects_the_rows_properties`
(reversion probe red for the right reason: `left: None / right:
Some("survives")`, `lane-logs/fround-f3red-1786470028.log`; green `166 tests
run: 166 passed`). GAP NOT YET CLOSED: the delete transition + properties
invariant are keystone work, fenced out of this lane.
