---
id: 2026-11-23-root-map-read-panicked-on-a-same-name-root
date: 2026-11-23
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Reading a typed-entity or type-declarations root map panicked when the doc
  also held a non-empty root of another kind under the same name.
---

## Bug
A lane verifier found it in a code audit: `held_root` in
`crates/holon-loro/src/loro_entity_operations.rs` panicked with "not a map"
when the root name did not resolve to a map. No automated test reached it.

## Root cause
Loro types a root by name and kind, so a List, MovableList, Text, Tree or
Counter root can share a map root's name. `get_by_path` by name picks the last
non-empty root of that name, so the lookup returned the other root. The boot
read of stored types and the projection of changed rows both went through
`held_root`.

## Missing piece
No generator drew a doc with a same-name root of another kind: the stored-type
restart PBT planted only map values, and no holon-loro test imported such a doc.

## Remedy
`resolve_root` reads the map by its typed id when a non-empty root of another
kind wins the name; the boot report names that root once and leaves it as it
is. Pinned by
`loro_entity_operations::tests::an_imported_root_of_another_kind_leaves_the_map_rows_readable`
and the `ForeignRoot` cases of `crates/holon-app/tests/runtime_type_survives_restart.rs`.
