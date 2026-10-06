---
id: 2026-10-06-malformed-block-id-on-a-structural-op-panics-at-the-dispatch-edge
date: 2026-10-06
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  An op parameter of type EntityUri holding a malformed id ("a b") panics at the
  shared macro dispatch edge instead of returning an error that names the
  parameter and the value; indent, outdent, move_up, move_down, delete_subtree
  and the other structural ops were all reachable.
---

## Bug
Found by a verifier (lane D69.a round 6, probe P4): `block.indent` with
`id = "a b"` through `execute_operation` panicked at
`crates/holon-api/src/entity_uri.rs:134` before the op ran. An empty id was
accepted as `block:`.

## Root cause
The `#[operations_trait]` macro lifted every `EntityUri` parameter with
`EntityUri::from_raw` (`crates/holon-macros/src/operations_trait.rs`, the
`EntityUri` branch of the parameter extraction). `from_raw` asserts that its
input forms a URI, so an id that comes from outside the system crashes the
dispatcher. The round-6 fix parsed ids at the `LoroBlockOperations` entries
only; the shared edge stayed open (`lane-logs/d69a-verify6.md`, "Other
findings").

## Missing piece
No generator or table test feeds a malformed or empty id to a structural op
through the dispatch edge; the keystone mints only well-formed ids.

## Remedy
The edge now calls `EntityUri::try_from_raw_param(param, value)`, which refuses
an empty or malformed value with an error naming the parameter and the value.
Test `unheld_write_tests::a_malformed_or_empty_id_on_a_structural_op_is_refused_by_name`
(`crates/holon-loro/src/loro_block_operations.rs`) covers every structural op
with {"a b", ""}; red log `lane-logs/r7-red-item1.log`.
