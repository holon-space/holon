---
id: 2026-10-06-malformed-edge-field-target-silently-accepted
date: 2026-10-06
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  set_field and create accepted an edge-field target such as requires: ["a b"]
  with Ok, although the string forms no entity URI.
---

## Bug
Found by a verifier (lane D69.a round 6, probe P4):
`set_field("block:held", "requires", ["a b"])` returned `Ok(OperationResult)`.

## Root cause
`set_field` stored the target strings in the tree node's edge meta key without
parsing them (`crates/holon-loro/src/loro_block_operations.rs`, the edge-field
arm of `CrudOperations::set_field`). `BlockEdges::set_from_raw`
(`crates/holon-api/src/edge_field.rs`) promoted strings with the panicking
`EntityUri::from_raw`, so the `create` path would have panicked on the same
input.

## Missing piece
No test writes a malformed edge target. Edge targets are references, not the
block's own id, so the round-6 id table did not cover them.

## Remedy
`set_from_raw` returns `Result` and refuses an empty or malformed reference
target with an error naming the field and the value; `set_field` runs the same
check before it writes. Test
`unheld_write_tests::a_malformed_edge_target_is_refused_by_name` covers
`requires`, `advice_suppressed` and `contributes_to` through `set_field` and
`create`; red log `lane-logs/r7-red-item2.log` (set_field returned Ok).
