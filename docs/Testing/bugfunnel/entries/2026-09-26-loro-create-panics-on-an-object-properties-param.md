---
id: 2026-09-26-loro-create-panics-on-an-object-properties-param
date: 2026-09-26
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A block `create` whose `properties` param is an Object (not a JSON string)
  panicked in the Loro block store instead of storing the properties or
  refusing the write.
---

## Bug
Found in lane `org-drawer-faithful` when the engine's multi-line value guard
was removed: the editing-suite case
`create_with_a_multi_line_value_in_the_properties_object_round_trips`
(`crates/holon-integration-tests/tests/editing_suite/multiline_property_write_boundary.rs`)
panics with `block.create 'properties' param must be a JSON string, got
Object({...})`. The guard used to refuse that write first, which hid the panic.

## Root cause
`crates/holon-loro/src/loro_block_operations.rs` (the `properties` flattening in
create) accepts only `Value::String` and panics on anything else. The engine
treats an Object `properties` as a valid shape (its key guard reads it).

## Missing piece
No generated or pinned case sends `create` with an Object `properties`.

## Remedy
`properties_bag` (`crates/holon-loro/src/loro_block_operations.rs`) parses the
param once, before the block is created: an Object or a JSON-object string
becomes the per-key map (the same two shapes `SqlOperationProvider` takes),
and any other value is refused with a named error, so no block is left
behind. Pinned by the Loro unit tests
`create_takes_the_properties_bag_as_an_object` and
`create_refuses_a_properties_bag_that_is_no_object`, and by the editing-suite
case `create_with_a_multi_line_value_in_the_properties_object_round_trips`
(red log `lane-logs/groupA-red2-engine.log`: the panic).
